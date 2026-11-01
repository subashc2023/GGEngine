//! Snapshot files: a run's world saved whole as text, which a run given `--restore` starts from, in
//! another process, after a crash or a rebuild, so that it goes on where the world was: its app,
//! its engine and build, its layers from the bottom up, the ticks the simulation had run, the
//! world's hash, the assets the layers had added, which name what's opaque in the world, the name
//! of each texture its run named otherwise than by its path, as by its ID, with that path, which
//! its values name textures by, read so in a run that names them otherwise, as after an ID file
//! changed, and taken as its own by a lockstep joiner, whose host's world comes so, the scene
//! files the layers had loaded, each its path, its author and its text as last applied, as a
//! recording's values write them, which the restored run watches, the open scene, its file's
//! author or none and the IDs it made, written so too, and the world, as [`World::save_whole`]
//! writes it; then, of a run the tool API had stopped, `stopped`, or of one playing a copy of the
//! world as edited, which it held aside, `edited`, the ticks that world was at and its hash,
//! followed by its scene files, its open scene and that world, written as the run's are; then
//! `end`. A snapshot written before the open scene was holds no `open` line: the start scene's
//! file is open, if its scene files hold it.
//!
//! ```text
//! gg-snapshot 1
//! app Sandbox
//! engine 0.1.0 debug
//! layers Sandbox PerfPanel Inspector
//! ticks 1234
//! hash 58183b6352a09a44 fb695ce29daa8479
//! asset texture 1 1 2 assets/checkerboard.png
//! named List([String("5f3c9a1e07b2d846"), String("assets/checkerboard.png")])
//! scene List([String("/game/scenes/grid.ron"), String("scenes/grid.ron"), String("(entities: ...")])
//! open List([Some(String("scenes/grid.ron")), List([String("1a2b3c4d5e6f7081")])])
//! stream 3f8a6c2e9b1d4057
//! slots 10530
//! free 12 7
//! table MapTile Sprite Transform
//! row 17 5f3a9c0e12b47d68 List([Struct([]), Struct([("color", Vec4(1.0, 1.0, 1.0, 1.0)), ...
//! children 5f3a9c0e12b47d68 1a2b3c4d5e6f7081
//! resource Physics2D (gravity:(0.0,-9.81),rapier:(...),linked:[],children:[])
//! end
//! ```

use std::collections::HashMap;
use std::fs;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use gg_core::{Error, Result};
use tracing::{info, warn};

use crate::recording::text::{Text, read_named};
use crate::reflect::Value;
use crate::scene_files::{Open, SceneFile};
use crate::{World, WorldHash};

/// A snapshot file's first line: its format, and the format's version, which a change to its lines
/// or to its values' text that old files can't be read by takes to the next.
const FORMAT: &str = "gg-snapshot 1";

/// What the run a snapshot is taken of is doing, which a run restored from it goes on doing.
pub(crate) enum Run {
    /// Playing from its start, holding no world as edited.
    Playing,
    /// Stopped, as the tool API's `stop` and `--stopped` leave it.
    #[cfg(feature = "dev")]
    Stopped,
    /// Playing a copy of the world as edited, which it holds aside, as the tool API's `play` has
    /// it: a copy of that world, its scene files as last applied, its open scene, and the ticks
    /// it's at.
    #[cfg(feature = "dev")]
    Held {
        world: Box<World>,
        scenes: Vec<SceneFile>,
        open: Open,
        ticks: u64,
    },
}

/// The text of the snapshot file of `world`, app `app`'s, whose layers are `layers`, from the bottom
/// up, as the simulation had run `ticks` ticks, with the scene files `scenes` as last applied and
/// the open scene `open`, as its run does what `run` says.
///
/// Fails as [`World::save_whole`] does.
pub(crate) fn write(
    world: &World,
    scenes: &[SceneFile],
    open: &Open,
    run: &Run,
    app: &str,
    layers: &[&str],
    ticks: u64,
) -> Result<String> {
    use std::fmt::Write as _;

    use crate::recording::engine;

    let _zone = gg_core::zone!();
    let mut out = String::new();
    let written = "a string takes any text";
    writeln!(out, "{FORMAT}").expect(written);
    writeln!(out, "app {app}").expect(written);
    writeln!(out, "engine {}", engine()).expect(written);
    writeln!(out, "layers {}", layers.join(" ")).expect(written);
    writeln!(out, "ticks {ticks}").expect(written);
    writeln!(out, "hash {}", world.hash()).expect(written);
    for added in world.assets().added() {
        writeln!(out, "asset {added}").expect(written);
    }
    for (name, path) in world.assets().named_from(0) {
        out.push_str("named ");
        crate::recording::text::write_named(&mut out, name, path);
        out.push('\n');
    }
    write_scenes(&mut out, scenes, open, world);
    world.save_whole(&mut out)?;
    match run {
        Run::Playing => {}
        #[cfg(feature = "dev")]
        Run::Stopped => out.push_str("stopped\n"),
        #[cfg(feature = "dev")]
        Run::Held {
            world,
            scenes,
            open,
            ticks,
        } => {
            writeln!(out, "edited {ticks} {}", world.hash()).expect(written);
            write_scenes(&mut out, scenes, open, world);
            world.save_whole(&mut out)?;
        }
    }
    out.push_str("end\n");
    Ok(out)
}

/// Writes a `scene` line after `out` for each of `scenes`, the scene files of `world`, but one
/// whose path isn't Unicode, which isn't watched after a restore; and the `open` line of `open`,
/// its open scene: untitled, if its file is one left out, which the restored run doesn't hold,
/// owning what the file's text holds as it owns what it made, in a development build, which alone
/// saves it.
fn write_scenes(out: &mut String, scenes: &[SceneFile], open: &Open, world: &World) {
    use crate::recording::text;

    for scene in scenes {
        if let Some(path) = scene.path.to_str() {
            let strings = [path, &scene.author, &scene.text];
            let strings = strings.map(|string| Value::String(string.to_owned()));
            out.push_str("scene ");
            text::write(out, &Value::List(strings.into()), world);
            out.push('\n');
        }
    }
    let left_out = (open.author.as_ref())
        .and_then(|author| scenes.iter().find(|scene| scene.author == *author))
        .filter(|scene| scene.path.to_str().is_none());
    #[cfg_attr(
        not(feature = "dev"),
        allow(unused_mut, reason = "only development builds add to it")
    )]
    let mut made = open.made.clone();
    #[cfg(feature = "dev")]
    if let Some(file) = left_out {
        // A text as last applied loaded then, so it reads as a scene's RON.
        made.extend(World::scene_ids(&file.text).unwrap_or_default());
        made.sort_unstable();
        made.dedup();
    }
    let author = open.author.as_ref().filter(|_| left_out.is_none());
    let author = author.map(|author| Box::new(Value::String(author.clone())));
    let made = made.iter().map(|id| Value::String(id.to_string()));
    let open = Value::List(vec![Value::Option(author), Value::List(made.collect())]);
    out.push_str("open ");
    text::write(out, &open, world);
    out.push('\n');
}

/// The open scene that `line`, the text after an `open` line's `open `, gives: its file's author,
/// or none, and the IDs it made, as [`write_scenes`] writes them.
fn read_open(line: &str) -> Option<Open> {
    let text = Text::parse(line, "open ".len() + 1).ok()?;
    let Value::List(parts) = text.value(&World::default()) else {
        return None;
    };
    let [Value::Option(author), Value::List(made)] = <[Value; 2]>::try_from(parts).ok()? else {
        return None;
    };
    let author = match author.map(|author| *author) {
        Some(Value::String(author)) => Some(author),
        Some(_) => return None,
        None => None,
    };
    let made = made.into_iter().map(|id| match id {
        Value::String(id) => id.parse().ok(),
        _ => None,
    });
    Some(Open {
        author,
        made: made.collect::<Option<_>>()?,
    })
}

/// The world's hash `text` gives: its content's and its layout's, in hex, as [`WorldHash`] writes
/// them.
fn world_hash(text: &str) -> Option<WorldHash> {
    let (content, layout) = text.split_once(' ')?;
    let hex = |text: &str| u64::from_str_radix(text, 16).ok();
    Some(WorldHash {
        content: hex(content)?,
        layout: hex(layout)?,
    })
}

/// Writes `text`, a snapshot file's, at `path`, whole or not at all, so that the last snapshot
/// there stays whole if it can't.
///
/// Fails with `snapshot.file` if it can't.
#[cfg(feature = "dev")]
pub(crate) fn save(path: &Path, text: &str) -> Result<()> {
    crate::dirs::write_whole(path, text.as_bytes()).map_err(|error| {
        let message = format!("can't write the snapshot {}", path.display());
        Error::new("snapshot.file", message).with_source(error)
    })
}

/// A snapshot file, as a run restored from it reads it as it starts: its header, checked, the lines
/// of its world, which load once the layers have, and what its run was doing.
pub(crate) struct Snapshot {
    /// Where it came from, as messages name it: its file's path, or a lockstep host.
    pub(crate) from: String,
    text: String,
    layers: Vec<String>,
    ticks: u64,
    hash: WorldHash,
    /// What the layers of the run it was taken from had added, in order, and the textures it loaded
    /// where they were named.
    assets: Vec<String>,
    /// The name that run gave each texture it named otherwise than by its path, by that path,
    /// which its values name textures by.
    #[cfg_attr(
        not(feature = "dev"),
        allow(
            dead_code,
            reason = "builds without `dev` name every texture by its path"
        )
    )]
    pub(crate) names: HashMap<Arc<str>, Arc<str>>,
    /// The scene files that run's layers had loaded, as last applied.
    pub(crate) scenes: Vec<SceneFile>,
    /// That run's open scene; `None` for a snapshot written before the open scene was.
    pub(crate) open: Option<Open>,
    /// The indexes of its world's lines.
    world: Range<usize>,
    /// Whether the run it was taken of was stopped.
    pub(crate) stopped: bool,
    /// The world as edited it holds, if it was taken while a copy of that world played.
    pub(crate) held: Option<Held>,
}

/// The world as edited that a snapshot taken while a copy of it played holds: the ticks it was at,
/// its hash, its scene files as last applied, its open scene, if the snapshot holds it, and the
/// indexes of its lines.
#[cfg_attr(
    not(feature = "dev"),
    allow(dead_code, reason = "only development builds hold one")
)]
pub(crate) struct Held {
    pub(crate) ticks: u64,
    hash: WorldHash,
    pub(crate) scenes: Vec<SceneFile>,
    pub(crate) open: Option<Open>,
    world: Range<usize>,
}

impl Snapshot {
    /// Reads the snapshot file at `path`, for app `app`, checking its header.
    ///
    /// Fails with `snapshot.file` if it can't be read; and with `snapshot.format` for a file that
    /// isn't a snapshot of this format, is another app's, or is cut short, its last line not `end`.
    pub(crate) fn read(path: &Path, app: &str) -> Result<Self> {
        let text = fs::read_to_string(path).map_err(|error| {
            let message = format!("can't read the snapshot {}", path.display());
            Error::new("snapshot.file", message).with_source(error)
        })?;
        Self::parse(text, path.display().to_string(), app)
    }

    /// Reads `text`, a snapshot file's, from where `from` names, for app `app`, checking its header
    /// and what follows its world: `stopped`, followed by `end` alone, or `edited`, the ticks and
    /// the hash of the world as edited, followed by its scene files, its open scene and its lines,
    /// the file holding no `stopped` or other `edited` after it; or `end`, of a run playing from
    /// its start.
    ///
    /// Fails with `snapshot.format` as [`read`](Self::read) does, for a scene file's or the open
    /// scene's line that isn't one, and for what follows its world otherwise.
    pub(crate) fn parse(text: String, from: String, app: &str) -> Result<Self> {
        let refused = |what: String| Error::new("snapshot.format", format!("{from}: {what}"));
        let lines: Vec<&str> = text.lines().collect();
        if lines.first() != Some(&FORMAT) {
            return Err(refused(format!(
                "it isn't a snapshot of this engine's, whose first line is `{FORMAT}`"
            )));
        }
        if lines.last() != Some(&"end") {
            return Err(refused(
                "it's cut short: its last line isn't `end`".to_owned(),
            ));
        }
        let field = |at: usize, key: &str| {
            let line = lines.get(at).and_then(|line| line.strip_prefix(key));
            let value =
                line.and_then(|line| line.strip_prefix(' ').or((line.is_empty()).then_some("")));
            value.ok_or_else(|| refused(format!("line {}: it isn't `{key} ...`", at + 1)))
        };
        let theirs = field(1, "app")?;
        if theirs != app {
            return Err(refused(format!("it's app `{theirs}`'s, not `{app}`'s")));
        }
        let layers = field(3, "layers")?.split_whitespace().map(str::to_owned);
        let ticks = field(4, "ticks")?;
        let ticks = ticks
            .parse()
            .map_err(|_| refused(format!("`{ticks}` isn't a number of ticks")))?;
        let hash = field(5, "hash")?;
        let hash =
            world_hash(hash).ok_or_else(|| refused(format!("`{hash}` isn't a world's hash")))?;
        let mut world = 6;
        let mut assets = Vec::new();
        while let Some(added) = lines
            .get(world)
            .and_then(|line| line.strip_prefix("asset "))
        {
            assets.push(added.to_owned());
            world += 1;
        }
        let mut names = HashMap::new();
        while let Some(named) = lines
            .get(world)
            .and_then(|line| line.strip_prefix("named "))
        {
            let (name, path) = read_named(named, "named ".len() + 1).ok_or_else(|| {
                refused(format!(
                    "line {}: a texture's name is a list of it and the path it names",
                    world + 1
                ))
            })?;
            names.insert(path, name);
            world += 1;
        }
        // The scene lines from line `at` on, then the open scene's line, if there's one, and the
        // index of the line after them.
        let read_scenes = |mut at: usize| {
            let mut scenes = Vec::new();
            while let Some(scene) = lines.get(at).and_then(|line| line.strip_prefix("scene ")) {
                let read = Text::parse(scene, "scene ".len() + 1)
                    .ok()
                    .and_then(|text| {
                        let Value::List(strings) = text.value(&World::default()) else {
                            return None;
                        };
                        let [
                            Value::String(path),
                            Value::String(author),
                            Value::String(text),
                        ] = <[Value; 3]>::try_from(strings).ok()?
                        else {
                            return None;
                        };
                        Some(SceneFile::new(path.into(), author, text))
                    });
                let scene = read.ok_or_else(|| {
                    refused(format!(
                        "line {}: a scene file is a list of its path, its author and its text",
                        at + 1
                    ))
                })?;
                scenes.push(scene);
                at += 1;
            }
            let mut open = None;
            if let Some(line) = lines.get(at).and_then(|line| line.strip_prefix("open ")) {
                open = Some(read_open(line).ok_or_else(|| {
                    refused(format!(
                        "line {}: the open scene is a list of its file's author, or none, and the \
                         IDs it made",
                        at + 1
                    ))
                })?);
                at += 1;
            }
            Ok((scenes, open, at))
        };
        let (scenes, open, world) = read_scenes(world)?;
        // The world's lines end at `end`, `stopped` or `edited`, which none of them is or starts
        // with.
        let tails = |line: &str| line == "stopped" || line.starts_with("edited ");
        let end = (world..lines.len())
            .find(|&at| lines[at] == "end" || tails(lines[at]))
            .expect("the last line is `end`");
        let (mut stopped, mut held) = (false, None);
        if lines[end] == "stopped" {
            if end + 2 != lines.len() {
                let what = format!("line {}: `stopped` is followed by `end` alone", end + 1);
                return Err(refused(what));
            }
            stopped = true;
        } else if let Some(edited) = lines[end].strip_prefix("edited ") {
            let read = edited
                .split_once(' ')
                .and_then(|(ticks, hash)| Some((ticks.parse().ok()?, world_hash(hash)?)));
            let (ticks, hash) = read.ok_or_else(|| {
                refused(format!(
                    "line {}: `edited` is followed by the ticks and the hash of the world as \
                     edited",
                    end + 1
                ))
            })?;
            let (scenes, open, first) = read_scenes(end + 1)?;
            let last = lines.len() - 1;
            if let Some(at) = (first..last).find(|&at| tails(lines[at])) {
                return Err(refused(format!(
                    "line {}: a snapshot holds one world as edited, or is stopped, but not both",
                    at + 1
                )));
            }
            held = Some(Held {
                ticks,
                hash,
                scenes,
                open,
                world: first..last,
            });
        }
        Ok(Self {
            from,
            layers: layers.collect(),
            ticks,
            hash,
            assets,
            names,
            scenes,
            open,
            world: world..end,
            stopped,
            held,
            text,
        })
    }

    /// The path of each texture the run it was taken from had loaded, as its `asset` lines give
    /// them, in order: which a run restored from it loads, where its layers haven't, before its
    /// world, as [`Context::load_restored`](crate::Context::load_restored) does.
    pub(crate) fn textures(&self) -> impl Iterator<Item = &str> {
        let textures = self
            .assets
            .iter()
            .filter_map(|line| line.strip_prefix("texture "));
        // The meshes, shaders and textures there were after it, then its path.
        textures.filter_map(|line| line.splitn(4, ' ').nth(3))
    }

    /// Has the snapshot name, by the path its file is at now, each texture its run named by an ID
    /// that `catalog`, the asset database of the run restored from it, holds for an asset at another
    /// path, its file gone from the path it had, as after a rename that the snapshot's run, or this
    /// run's start, moved its ID file for: in its `asset` lines and its names, so that the run loads
    /// the texture from there, and its own `asset` line for it, which then is the snapshot's, keeps
    /// what's opaque in the world. A texture whose file is where it was keeps its path, though a
    /// copy took its ID, as its ID changed since. A model's images, which load with their model,
    /// keep their paths.
    #[cfg(feature = "dev")]
    pub(crate) fn follow(&mut self, catalog: &crate::assets::Catalog) {
        let moved: HashMap<Arc<str>, Arc<str>> = (self.names.iter())
            .filter_map(|(path, name)| {
                let now = catalog.path(name.parse().ok()?)?;
                (now != &**path && !catalog.present(path)).then(|| (path.clone(), now.into()))
            })
            .collect();
        if moved.is_empty() {
            return;
        }
        for line in &mut self.assets {
            let Some(texture) = line.strip_prefix("texture ") else {
                continue;
            };
            let mut parts = texture.splitn(4, ' ');
            let counts: Vec<&str> = parts.by_ref().take(3).collect();
            if let Some(now) = parts.next().and_then(|path| moved.get(path)) {
                *line = format!("texture {} {now}", counts.join(" "));
            }
        }
        let names = self.names.drain().map(|(path, name)| {
            let now = moved.get(&path).cloned().unwrap_or(path);
            (now, name)
        });
        self.names = names.collect();
    }

    /// Loads the snapshot's world into `world`, whose component types and assets the layers,
    /// `layers` from the bottom up, have registered and added as they loaded, in place of its
    /// entities, as [`World::load_whole`] does; what's opaque in it comes back if the layers added
    /// the assets the snapshot's had, in the same order, as their first; and each texture its
    /// values name by a name its run gave it, as by an ID since changed, comes back by the path
    /// that name was given, the world hashed as that run named it to check it against the hash
    /// saved, though the run goes on naming it its own way. Logs what it changed of
    /// the world to match the types as they are, each change once, and returns the ticks the
    /// simulation had run, which the run goes on from.
    ///
    /// Fails as `load_whole` does.
    pub(crate) fn restore(&self, world: &mut World, layers: &[&str]) -> Result<u64> {
        let path = &self.from;
        if !self
            .layers
            .iter()
            .map(String::as_str)
            .eq(layers.iter().copied())
        {
            warn!(
                code = "snapshot.other_layers",
                path = %path,
                "The snapshot is of layers `{}`, from the bottom up, but the app's are `{}`: its \
                 world is restored all the same",
                self.layers.join(" "),
                layers.join(" ")
            );
        }
        self.load(world, self.world.clone(), self.hash, "world")?;
        let entities = world.entities(None).map_or(0, |entities| entities.len());
        info!(path = %path, ticks = self.ticks, entities, "Restored the world from a snapshot");
        Ok(self.ticks)
    }

    /// Loads the world as edited that the snapshot holds, if it was taken while a copy of that
    /// world played, into `world`, a copy of the world as the layers loaded it, as
    /// [`restore`](Self::restore) loads the run's; nothing for a snapshot that holds none.
    ///
    /// Fails as `restore` does.
    #[cfg(feature = "dev")]
    pub(crate) fn restore_held(&self, world: &mut World) -> Result<()> {
        let Some(held) = &self.held else {
            return Ok(());
        };
        self.load(world, held.world.clone(), held.hash, "world as edited")?;
        let entities = world.entities(None).map_or(0, |entities| entities.len());
        info!(
            path = %self.from,
            ticks = held.ticks,
            entities,
            "Restored the world as edited from a snapshot"
        );
        Ok(())
    }

    /// Loads the world whose lines are those at `lines`, as [`restore`](Self::restore) says,
    /// checking it against `hash`, the hash it was saved with, and naming it `what` as it warns.
    ///
    /// Fails as [`World::load_whole`] does.
    fn load(
        &self,
        world: &mut World,
        lines: Range<usize>,
        hash: WorldHash,
        what: &str,
    ) -> Result<()> {
        let path = &self.from;
        let added = world.assets().added();
        let same = self.assets.len() <= added.len()
            && (self.assets.iter())
                .zip(added)
                .all(|(theirs, ours)| *theirs == **ours);
        let numbered =
            (lines.start + 1..).zip(self.text.lines().skip(lines.start).take(lines.len()));
        // Its values and its hash name textures as its run did, so a run that names them otherwise,
        // as after an ID file changed, reads and hashes them by those names for the load, by the
        // paths they were given, and goes on with its own.
        #[cfg(feature = "dev")]
        let own = {
            let own = Arc::clone(world.shared_assets());
            let alike = own.named_from(0).count() == self.names.len()
                && (own.named_from(0)).all(|(name, path)| {
                    self.names.get(path).is_some_and(|theirs| **theirs == *name)
                });
            if !alike {
                let mut theirs = (*own).clone();
                theirs.rename(&self.names);
                world.share_assets(&Arc::new(theirs));
            }
            own
        };
        let loaded = world.load_whole(numbered, same);
        let restored = loaded.as_ref().ok().map(|_| world.hash());
        #[cfg(feature = "dev")]
        world.share_assets(&own);
        let changes = loaded.map_err(|error| {
            let message = format!("{path}: {error:#}");
            Error::new(error.code(), message)
        })?;
        for (change, count) in changes.iter() {
            warn!(
                code = "snapshot.changed",
                path = %path,
                count,
                "The {what} is restored otherwise than it was saved, to match its types as they \
                 are: {change}"
            );
        }
        let restored = restored.expect("hashed once loaded");
        if changes.is_empty() && restored != hash {
            warn!(
                code = "snapshot.hash",
                path = %path,
                "The {what} restored hashes {restored}, where the {what} saved hashed {hash}: it \
                 goes on otherwise than it would have"
            );
        }
        Ok(())
    }
}

// They write snapshots, which only development builds do.
#[cfg(all(test, feature = "dev"))]
mod tests {
    use super::*;
    use crate::dirs::tests::Scratch;
    use crate::layer::Stack;
    use crate::{Context, Layer, MeshDraw, Sprite, Transform};

    /// A snapshot written and read back restores the world it was taken of, at its ticks, into a
    /// world its layers loaded, with its scene files and its open scene; one written before the
    /// open scene was holds none; and one of another app, of another format, cut short, or whose
    /// open scene's line isn't one, is refused.
    #[test]
    fn a_snapshot_restores_the_world_it_was_taken_of() {
        let mut saved = Context::seeded(1);
        let world = saved.world_mut();
        let made = world.spawn((Transform::IDENTITY, Sprite::default()));
        // A path and a text that need escapes.
        let scene = SceneFile::new(
            "scenes/a \"grid\".ron".into(),
            "scenes/grid.ron".to_owned(),
            "(entities: {\n})\n".to_owned(),
        );
        let scenes = [scene];
        let open = Open {
            author: Some("scenes/grid.ron".to_owned()),
            made: vec![saved.world().id(made).expect("alive")],
        };
        let text = write(
            saved.world(),
            &scenes,
            &open,
            &Run::Playing,
            "Tested",
            &["Tester"],
            77,
        );
        let text = text.expect("written");
        let thread = std::thread::current().id();
        let dir = Scratch::new(&format!("snapshot-{thread:?}"));
        fs::create_dir_all(&dir.0).expect("a directory");
        let path = dir.0.join("tested.snapshot");
        save(&path, &text).expect("saved");
        let snapshot = Snapshot::read(&path, "Tested").expect("read");
        assert_eq!(
            (&snapshot.scenes[..], &snapshot.open),
            (&scenes[..], &Some(open))
        );
        let open_line = text.lines().find(|line| line.starts_with("open "));
        let open_line = format!("{}\n", open_line.expect("an open line"));
        let older = text.replace(&open_line, "");
        let older = Snapshot::parse(older, "older".to_owned(), "Tested").expect("read");
        assert_eq!(older.open, None);
        let mut restored = Context::seeded(2);
        let ticks = snapshot.restore(restored.world_mut(), &["Tester"]);
        assert_eq!(ticks.expect("restored"), 77);
        assert_eq!(restored.world().hash(), saved.world().hash());
        let refused = Snapshot::read(&path, "Other")
            .err()
            .map(|error| error.code().to_owned());
        assert_eq!(refused.as_deref(), Some("snapshot.format"));
        for broken in [
            text.replace(FORMAT, "gg-snapshot 0"),
            text.replace("\nend\n", "\n"),
            text.replace(&open_line, "open List([None])\n"),
            text.replace(&open_line, "open List([None, List([String(\"nope\")])])\n"),
        ] {
            save(&path, &broken).expect("saved");
            let refused = Snapshot::read(&path, "Tested")
                .err()
                .map(|error| error.code().to_owned());
            assert_eq!(refused.as_deref(), Some("snapshot.format"), "{broken}");
        }
        let missing = Snapshot::read(&dir.0.join("none.snapshot"), "Tested");
        assert_eq!(
            missing
                .err()
                .map(|error| error.code().to_owned())
                .as_deref(),
            Some("snapshot.file")
        );
    }

    /// The open scene of a file whose path isn't Unicode, which a snapshot leaves out, is written
    /// untitled, owning what the file's text holds as it owns what it made.
    #[cfg(any(windows, unix))]
    #[test]
    fn the_open_scene_of_a_file_left_out_is_written_untitled() {
        // A path that isn't Unicode, as one on disk may be.
        #[cfg(windows)]
        let path = {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[0x61, 0xD800, 0x2E, 0x72, 0x6F, 0x6E])
        };
        #[cfg(unix)]
        let path = {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(b"a\xFF.ron".to_vec())
        };
        let mut world = World::seeded(1);
        let [a, b] = [0, 1].map(|_| world.spawn(Transform::IDENTITY));
        let text = world.save_scene(&[a]).expect("saved");
        let [a, b] = [a, b].map(|entity| world.id(entity).expect("alive"));
        let scenes = [SceneFile::new(path.into(), "scenes/a.ron".to_owned(), text)];
        let open = Open {
            author: Some("scenes/a.ron".to_owned()),
            made: vec![b],
        };
        let text = write(&world, &scenes, &open, &Run::Playing, "Tested", &[], 0);
        let snapshot = Snapshot::parse(text.expect("written"), "left".to_owned(), "Tested");
        let snapshot = snapshot.expect("read");
        assert!(snapshot.scenes.is_empty());
        let mut made = vec![a, b];
        made.sort_unstable();
        assert_eq!(snapshot.open, Some(Open { author: None, made }));
    }

    /// A snapshot taken while a copy of the world as edited played holds that world too, which
    /// comes back, at its ticks, with its scene files, beside the copy's; one taken stopped says
    /// so, and holds the run's world alone; one of a run playing from its start says neither; and
    /// one that's stopped and holds a world as edited, or whose `stopped` or `edited` line is
    /// broken, is refused.
    #[test]
    fn a_snapshot_keeps_what_its_run_was_doing() {
        let mut edited = Context::seeded(1);
        edited
            .world_mut()
            .spawn((Transform::IDENTITY, Sprite::default()));
        let mut copy = edited.world().clone();
        copy.spawn(Transform::IDENTITY);
        let scene = |author: &str| {
            let path = format!("scenes/{author}.ron");
            SceneFile::new(
                path.into(),
                author.to_owned(),
                "(entities: {\n})\n".to_owned(),
            )
        };
        let open = |author: &str| Open {
            author: Some(author.to_owned()),
            made: Vec::new(),
        };
        let run = Run::Held {
            world: Box::new(edited.world().clone()),
            scenes: vec![scene("edited")],
            open: open("edited"),
            ticks: 40,
        };
        let (scenes, opened) = ([scene("copy")], open("copy"));
        let text = write(&copy, &scenes, &opened, &run, "Tested", &["Tester"], 77);
        let held_text = text.expect("written");
        let snapshot = Snapshot::parse(held_text.clone(), "held".to_owned(), "Tested");
        let snapshot = snapshot.expect("read");
        assert!(!snapshot.stopped);
        assert_eq!(snapshot.scenes, [scene("copy")]);
        assert_eq!(snapshot.open, Some(open("copy")));
        let held = snapshot.held.as_ref().expect("the world as edited");
        assert_eq!((held.ticks, &held.scenes[..]), (40, &[scene("edited")][..]));
        assert_eq!(held.open, Some(open("edited")));
        let [mut running, mut kept] = [2, 3].map(Context::seeded);
        let ticks = snapshot.restore(running.world_mut(), &["Tester"]);
        assert_eq!(ticks.expect("restored"), 77);
        snapshot.restore_held(kept.world_mut()).expect("restored");
        assert_eq!(running.world().hash(), copy.hash());
        assert_eq!(kept.world().hash(), edited.world().hash());
        let none = Open::default();
        let stopped_text = write(&copy, &[], &none, &Run::Stopped, "Tested", &[], 0);
        let stopped_text = stopped_text.expect("written");
        let snapshot = Snapshot::parse(stopped_text.clone(), "stopped".to_owned(), "Tested");
        let snapshot = snapshot.expect("read");
        assert!(snapshot.stopped && snapshot.held.is_none());
        let mut running = Context::seeded(4);
        snapshot
            .restore(running.world_mut(), &[])
            .expect("restored");
        assert_eq!(running.world().hash(), copy.hash());
        let playing = write(&copy, &[], &none, &Run::Playing, "Tested", &[], 0).expect("written");
        let snapshot = Snapshot::parse(playing, "playing".to_owned(), "Tested").expect("read");
        assert!(!snapshot.stopped && snapshot.held.is_none());
        for broken in [
            held_text.replace("\nend\n", "\nstopped\nend\n"),
            stopped_text.replace("stopped\nend\n", "stopped\nedited 0 0 0\nend\n"),
            held_text.replace("edited 40 ", "edited forty "),
        ] {
            let refused = Snapshot::parse(broken.clone(), "broken".to_owned(), "Tested");
            let refused = refused.err().map(|error| error.code().to_owned());
            assert_eq!(refused.as_deref(), Some("snapshot.format"), "{broken}");
        }
    }

    /// A layer that adds a mesh as it loads, whose vertices are `N` floats.
    struct Drawn<const N: usize>;

    impl<const N: usize> Layer for Drawn<N> {
        fn load(&mut self, cx: &mut Context) {
            cx.add_mesh("tri", &[[0.0f32; N]; 3], &[0, 1, 2])
                .expect("a mesh");
        }
    }

    /// What's opaque, as a `MeshDraw` is, comes back into a run whose layers added the same assets
    /// in the same order as the run it was taken in, and is left out otherwise, as where a mesh's
    /// vertices are of another type, which its draws' bytes would be taken for.
    #[test]
    fn what_is_opaque_comes_back_where_the_same_assets_were_added() {
        let mut saved = Context::seeded(3);
        Drawn::<3>.load(&mut saved);
        let quad = MeshDraw::from_parts(0, 0, false, false, &[]).expect("a draw");
        let drawn = saved.world_mut().spawn((Transform::IDENTITY, quad));
        let none = Open::default();
        let text = write(saved.world(), &[], &none, &Run::Playing, "Tested", &[], 0);
        let text = text.expect("written");
        let thread = std::thread::current().id();
        let dir = Scratch::new(&format!("snapshot-opaque-{thread:?}"));
        fs::create_dir_all(&dir.0).expect("a directory");
        let path = dir.0.join("drawn.snapshot");
        save(&path, &text).expect("saved");
        let snapshot = Snapshot::read(&path, "Tested").expect("read");
        let mut same = Context::seeded(4);
        Drawn::<3>.load(&mut same);
        snapshot.restore(same.world_mut(), &[]).expect("restored");
        assert_eq!(same.world().get::<MeshDraw>(drawn), Some(&quad));
        let mut wider = Context::seeded(5);
        Drawn::<4>.load(&mut wider);
        for mut other in [Context::seeded(5), wider] {
            snapshot.restore(other.world_mut(), &[]).expect("restored");
            assert_eq!(other.world().get::<MeshDraw>(drawn), None);
            assert_eq!(
                other.world().get::<Transform>(drawn),
                Some(&Transform::IDENTITY)
            );
        }
    }

    /// A snapshot names each texture its run named by an ID with its path, and its values hold
    /// textures by those names; a run whose ID files give the texture another ID since, as after a
    /// checkout, restores each to its texture all the same, its hash as the snapshot's run named
    /// it the one saved, so that it warns of no change, and goes on by the run's own name, and so
    /// hashes otherwise.
    #[test]
    fn a_texture_comes_back_by_the_name_its_snapshot_gave_it() {
        let logged = crate::testing::logged();
        let dir = Scratch::new("snapshot-named");
        let root = dir.0.clone();
        fs::create_dir_all(root.join("assets")).expect("a directory");
        let png =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        fs::write(root.join("assets/stone.png"), png).expect("a PNG");
        let named = |id: &str| {
            let meta = gg_api::asset::Meta {
                id: id.parse().expect("an ID"),
            };
            fs::write(root.join("assets/stone.png.meta"), meta.text()).expect("its ID file");
            let mut cx = Context::seeded(3).root(root.clone());
            let stone = cx.add_texture("stone", root.join("assets/stone.png"));
            (cx, stone.expect("a texture"))
        };
        let (mut saved, stone) = named("5f3c9a1e07b2d846");
        let sprite = Sprite {
            texture: Some(stone),
            ..Sprite::default()
        };
        let shown = saved.world_mut().spawn((Transform::IDENTITY, sprite));
        let none = Open::default();
        let text = write(saved.world(), &[], &none, &Run::Playing, "Tested", &[], 0);
        let text = text.expect("written");
        let line = "named List([String(\"5f3c9a1e07b2d846\"), String(\"assets/stone.png\")])";
        assert!(text.lines().any(|named| named == line), "{text}");
        assert!(text.contains("Texture(\"5f3c9a1e07b2d846\")"), "{text}");
        let snapshot = Snapshot::parse(text, "a test".to_owned(), "Tested").expect("read");
        let (mut restored, stone) = named("0a1b2c3d4e5f6071");
        let _ = logged.codes.try_iter().count();
        snapshot
            .restore(restored.world_mut(), &[])
            .expect("restored");
        let codes: Vec<String> = logged.codes.try_iter().collect();
        let warned = codes.iter().filter(|code| code.starts_with("snapshot."));
        assert_eq!(warned.count(), 0, "{codes:?}");
        let world = restored.world();
        assert_eq!(world.get::<Sprite>(shown), Some(&sprite));
        assert_eq!(world.assets().name(stone), Some("0a1b2c3d4e5f6071"));
        assert_ne!(world.hash(), saved.world().hash());
    }

    /// A texture the snapshot's run loaded where text named it, rather than as a layer loaded, is
    /// loaded again, by the path its `asset` line gives, by a run restored from it, whose layers
    /// don't load it; or, its file gone, stands in, named as the snapshot names it, so that the
    /// world restored hashes as the one saved either way, warning of no change. A run that follows
    /// another finds the file of an ID such a line gives, as a stand-in's does, by its own
    /// database, the texture named by the ID still.
    #[test]
    fn a_texture_loaded_where_named_comes_back_or_stands_in() {
        let logged = crate::testing::logged();
        let dir = Scratch::new("snapshot-loaded");
        let root = dir.0.clone();
        fs::create_dir_all(root.join("assets")).expect("a directory");
        let png =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        fs::write(root.join("assets/stone.png"), png).expect("a PNG");
        let id = "5f3c9a1e07b2d846";
        let meta = gg_api::asset::Meta {
            id: id.parse().expect("an ID"),
        };
        fs::write(root.join("assets/stone.png.meta"), meta.text()).expect("its ID file");
        let mut saved = Context::seeded(3).root(root.clone());
        saved.load_named([id], false);
        let stone = saved.world().assets().texture(id).expect("loaded");
        let sprite = Sprite {
            texture: Some(stone),
            ..Sprite::default()
        };
        let shown = saved.world_mut().spawn((Transform::IDENTITY, sprite));
        let none = Open::default();
        let text = write(saved.world(), &[], &none, &Run::Playing, "Tested", &[], 0);
        let snapshot = Snapshot::parse(text.expect("written"), "a test".to_owned(), "Tested");
        let snapshot = snapshot.expect("read");
        assert_eq!(
            snapshot.textures().collect::<Vec<_>>(),
            ["assets/stone.png"]
        );
        // A run that follows another, given no name for the file, finds by its own database an ID
        // an `asset` line gives, as a stand-in's names one, the texture still named by the ID.
        let mut following = Context::seeded(3).root(root.clone());
        following.name_as([]);
        following.load_restored([id]);
        let assets = following.world().assets();
        let found = assets.texture(id).and_then(|texture| assets.name(texture));
        assert_eq!(found, Some(id));

        for (gone, stood_in) in [(false, 0), (true, 1)] {
            if gone {
                fs::remove_file(root.join("assets/stone.png")).expect("removed");
            }
            let mut restored = Context::seeded(3).root(root.clone());
            let _ = logged.codes.try_iter().count();
            let restoring = Stack::default().restore(&mut restored, &snapshot, false);
            restoring.expect("restored");
            let codes: Vec<String> = logged.codes.try_iter().collect();
            let warned = codes.iter().filter(|code| code.starts_with("snapshot."));
            assert_eq!(warned.count(), 0, "{codes:?}");
            let stand_ins = codes.iter().filter(|code| *code == "asset.stand_in");
            assert_eq!(stand_ins.count(), stood_in, "{codes:?}");
            let world = restored.world();
            let texture = world.get::<Sprite>(shown).and_then(|sprite| sprite.texture);
            assert!(texture.is_some_and(|texture| world.assets().path(texture).is_some()));
        }
    }

    /// A texture loaded where named, its file renamed since with its ID file, as the watcher moves
    /// it, comes back from where its file is now in a run whose database holds its ID there, once
    /// the snapshot follows it, the world restored warning of no change: its `asset` line, which
    /// follows it too, is the one the run's load notes. One whose file is where it was, a copy
    /// having taken its ID, keeps its path.
    #[test]
    fn a_texture_renamed_since_comes_back_from_where_its_file_is() {
        let logged = crate::testing::logged();
        let dir = Scratch::new("snapshot-renamed");
        let root = dir.0.clone();
        fs::create_dir_all(root.join("assets")).expect("a directory");
        let png =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        fs::write(root.join("assets/stone.png"), png).expect("a PNG");
        let id = "5f3c9a1e07b2d846";
        let meta = gg_api::asset::Meta {
            id: id.parse().expect("an ID"),
        };
        fs::write(root.join("assets/stone.png.meta"), meta.text()).expect("its ID file");
        let mut saved = Context::seeded(3).root(root.clone());
        saved.load_named([id], false);
        let stone = saved.world().assets().texture(id).expect("loaded");
        let sprite = Sprite {
            texture: Some(stone),
            ..Sprite::default()
        };
        let shown = saved.world_mut().spawn((Transform::IDENTITY, sprite));
        let none = Open::default();
        let text = write(saved.world(), &[], &none, &Run::Playing, "Tested", &[], 0);
        let text = text.expect("written");
        let assets = root.join("assets");
        let parsed = || Snapshot::parse(text.clone(), "a test".to_owned(), "Tested").expect("read");
        // A copy made with its ID file, which a run's first scan gives the ID to, as the first by
        // path whose ID file holds it, while the texture's file is where it was: no rename.
        for name in ["stone.png", "stone.png.meta"] {
            fs::copy(assets.join(name), assets.join(format!("a_{name}"))).expect("copied");
        }
        let copied = Context::seeded(3).root(root.clone());
        let mut kept = parsed();
        kept.follow(copied.catalog());
        assert_eq!(kept.textures().collect::<Vec<_>>(), ["assets/stone.png"]);
        for name in ["a_stone.png", "a_stone.png.meta"] {
            fs::remove_file(assets.join(name)).expect("removed");
        }
        for name in ["stone.png", "stone.png.meta"] {
            let to = assets.join(name.replace("stone", "pillar"));
            fs::rename(assets.join(name), to).expect("moved");
        }

        let mut restored = Context::seeded(3).root(root.clone());
        let mut snapshot = parsed();
        snapshot.follow(restored.catalog());
        assert_eq!(
            snapshot.textures().collect::<Vec<_>>(),
            ["assets/pillar.png"]
        );
        let _ = logged.codes.try_iter().count();
        let restoring = Stack::default().restore(&mut restored, &snapshot, false);
        restoring.expect("restored");
        let codes: Vec<String> = logged.codes.try_iter().collect();
        let warned = codes.iter().filter(|code| code.starts_with("snapshot."));
        assert_eq!(warned.count(), 0, "{codes:?}");
        assert!(
            !codes.iter().any(|code| code == "asset.stand_in"),
            "{codes:?}"
        );
        let world = restored.world();
        let texture = world.get::<Sprite>(shown).and_then(|sprite| sprite.texture);
        let path = texture.and_then(|texture| world.assets().path(texture));
        assert_eq!(path, Some("assets/pillar.png"));
        assert_eq!(world.hash(), saved.world().hash());
    }
}
