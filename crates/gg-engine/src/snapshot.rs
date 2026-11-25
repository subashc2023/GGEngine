//! Snapshot files: a run's world saved whole as text. A run given `--restore` starts from one, in
//! another process, after a crash or rebuild, and goes on where the world was. In order, a file
//! holds:
//!
//! - the app, the engine and build, and the layers from the bottom up;
//! - the ticks run, and the world's hash;
//! - the assets the layers added and removed, which name what's opaque in the world, and the
//!   shaders and meshes the engine added when first asked for, among them in order;
//! - each texture's name where its run named it otherwise than by path (as by ID), with the path.
//!   Values name textures by these. A run that names them otherwise (after an ID file changed)
//!   reads them so; a lockstep joiner takes them as its own, as its host's world comes so;
//! - the scripts the layers loaded: path from the project's root, text as last applied. A lockstep
//!   joiner runs these in place of its own; a restored run reads its files;
//! - the mods: manifest text, then each script's path in its folder and text. A lockstep joiner
//!   runs these in place of its own; a restored run loads those installed;
//! - the scene files: path, author and text as last applied, written as a recording's values are.
//!   The restored run watches them;
//! - the open scene: its file's author or none, and the IDs it made, written likewise;
//! - the world, as [`World::save_whole`] writes it;
//! - `stopped` for a run the tool API stopped, or, for one playing a copy of the world as edited
//!   (held aside), `edited`, that world's ticks and hash, then its scene files, open scene and
//!   world, written as the run's are;
//! - `paused` if its simulation was, as the tool API's `pause` leaves it;
//! - `end`.
//!
//! A snapshot written before the open scene was holds no `open` line: the start scene's file is
//! open, if its scene files hold it.
//!
//! ```text
//! gg-snapshot 2
//! app Sandbox
//! engine 0.1.0 debug
//! layers Sandbox PerfPanel Inspector
//! ticks 1234
//! hash 58183b6352a09a44 fb695ce29daa8479
//! asset engine shader 1 text
//! asset texture 1 2 2 assets/checkerboard.png
//! named List([String("5f3c9a1e07b2d846"), String("assets/checkerboard.png")])
//! script List([String("scripts/spin.luau"), String("local Spin = ...")])
//! mod List([String("(name: \"rainbow\", ..."), String("rainbow.luau"), String("local ...")])
//! scene List([String("/game/scenes/grid.ron"), String("scenes/grid.ron"), String("(entities: ...")])
//! open List([Some(String("scenes/grid.ron")), List([String("1a2b3c4d5e6f7081")])])
//! stream 3f8a6c2e9b1d4057
//! slots 10530
//! free 12 7
//! vacant 3
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

use crate::World;
use crate::assets::Asset;
use crate::cells::Seen;
use crate::ecs::WorldHash;
use crate::luau::ModText;
use crate::recording::text::{Text, read_mod, read_named};
use crate::reflect::Value;
use crate::scene_files::{Open, SceneFile};

/// A snapshot file's first line: the format and its version. A change to its lines or its values'
/// text that old files can't be read by bumps the version.
const FORMAT: &str = "gg-snapshot 2";

/// What a snapshot's run is doing; a restored run goes on doing it.
pub(crate) enum Run {
    /// Playing from its start, holding no world as edited.
    Playing,
    /// Stopped, as the tool API's `stop` and `--stopped` leave it.
    #[cfg(feature = "dev")]
    Stopped,
    /// Playing a copy of the world as edited, which it holds aside (the tool API's `play`), with
    /// that world's scene files as last applied, open scene and ticks.
    #[cfg(feature = "dev")]
    Held {
        world: Box<World>,
        scenes: Vec<SceneFile>,
        open: Open,
        ticks: u64,
    },
}

/// What a snapshot is of: a run's world, scene files as last applied, open scene, scripts (path
/// and text as last applied) and mods.
pub(crate) struct Of<'r> {
    pub(crate) world: &'r World,
    pub(crate) scenes: &'r [SceneFile],
    /// The scene files' texts the run keeps for scene changes
    /// ([`Context::scene_text`](crate::Context::scene_text)).
    pub(crate) cached: &'r [SceneFile],
    pub(crate) open: &'r Open,
    pub(crate) scripts: &'r [(String, String)],
    pub(crate) mods: &'r [ModText],
    /// What a server's client sees: written and hashed as [`World::save_seen`] and
    /// [`World::hash_seen`] say, without the cells' file texts, which a client never reads. `None`
    /// for the world whole.
    pub(crate) seen: Option<&'r Seen>,
}

/// The snapshot text of `of` for app `app` with `layers` from the bottom up, after `ticks` ticks,
/// its run doing `run`, its simulation not paused.
///
/// Fails as [`World::save_whole`] does.
pub(crate) fn write(of: &Of, run: &Run, app: &str, layers: &[&str], ticks: u64) -> Result<String> {
    write_paused(of, run, false, app, layers, ticks)
}

/// Like [`write()`], with the simulation paused if `paused` (the file's `paused` line).
///
/// Fails as `write` does.
pub(crate) fn write_paused(
    of: &Of,
    run: &Run,
    paused: bool,
    app: &str,
    layers: &[&str],
    ticks: u64,
) -> Result<String> {
    use std::fmt::Write as _;

    use crate::recording::engine;

    let _zone = gg_core::zone!();
    let Of {
        world,
        scenes,
        cached,
        open,
        scripts,
        mods,
        seen,
    } = *of;
    let mut out = String::new();
    let written = "a string takes any text";
    writeln!(out, "{FORMAT}").expect(written);
    writeln!(out, "app {app}").expect(written);
    writeln!(out, "engine {}", engine()).expect(written);
    writeln!(out, "layers {}", layers.join(" ")).expect(written);
    writeln!(out, "ticks {ticks}").expect(written);
    writeln!(out, "hash {}", world.hash_seen(seen)).expect(written);
    for added in world.assets().added() {
        writeln!(out, "asset {added}").expect(written);
    }
    for (name, path) in world.assets().named_from((0, 0, 0, 0)) {
        out.push_str("named ");
        crate::recording::text::write_named(&mut out, name, path);
        out.push('\n');
    }
    for (path, text) in scripts {
        let strings = [path, text].map(|string| Value::String(string.clone()));
        out.push_str("script ");
        crate::recording::text::write(&mut out, &Value::List(strings.into()), world);
        out.push('\n');
    }
    for text in mods {
        out.push_str("mod ");
        crate::recording::text::write_mod(&mut out, text);
        out.push('\n');
    }
    let folder = seen
        .and(world.world_cells())
        .map(|(_, cells, _)| &cells.folder);
    let cell_file = |scene: &SceneFile| {
        let name = folder.and_then(|folder| scene.author.strip_prefix(folder.as_str()));
        let name = name.and_then(|name| name.strip_prefix('/'));
        name.and_then(crate::cells::Cell::of_file).is_some()
    };
    for scene in cached.iter().filter(|scene| !cell_file(scene)) {
        if let Some(path) = scene.path.to_str() {
            let strings = [path, &scene.author, &scene.text];
            let strings = strings.map(|string| Value::String(string.to_owned()));
            out.push_str("cached ");
            crate::recording::text::write(&mut out, &Value::List(strings.into()), world);
            out.push('\n');
        }
    }
    write_scenes(&mut out, scenes, open, world);
    world.save_seen(&mut out, seen)?;
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
    if paused {
        out.push_str("paused\n");
    }
    out.push_str("end\n");
    Ok(out)
}

/// Writes a `scene` line per scene file of `scenes`, except one with a non-Unicode path (not
/// watched after a restore), then `open`'s `open` line. If the open file is a left-out one, the
/// scene is untitled and owns what the file's text holds as well as what it made; only
/// development builds save that.
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
    let mut parts = vec![Value::Option(author), Value::List(made.collect())];
    if open.edited {
        parts.push(Value::Bool(true));
    }
    let open = Value::List(parts);
    out.push_str("open ");
    text::write(out, &open, world);
    out.push('\n');
}

/// The open scene an `open` line's text `line` gives: file author or none, and IDs made
/// ([`write_scenes`]).
fn read_open(line: &str) -> Option<Open> {
    let text = Text::parse(line, "open ".len() + 1).ok()?;
    let Value::List(parts) = text.value(&World::default()) else {
        return None;
    };
    // Whether it was edited comes last; older snapshots omit it.
    let (author, made, edited) = match <[Value; 3]>::try_from(parts) {
        Ok([author, made, Value::Bool(edited)]) => (author, made, edited),
        Ok(_) => return None,
        Err(parts) => {
            let [author, made] = <[Value; 2]>::try_from(parts).ok()?;
            (author, made, false)
        }
    };
    let (Value::Option(author), Value::List(made)) = (author, made) else {
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
        edited,
    })
}

/// The world hash `text` gives: content and layout, in hex, as [`WorldHash`] writes them.
fn world_hash(text: &str) -> Option<WorldHash> {
    let (content, layout) = text.split_once(' ')?;
    let hex = |text: &str| u64::from_str_radix(text, 16).ok();
    Some(WorldHash {
        content: hex(content)?,
        layout: hex(layout)?,
    })
}

/// What a restored run takes again of an `asset` line
/// ([`Context::load_restored`](crate::Context::load_restored)):
/// - a texture, model or font to load, from the path after the mesh, shader and texture counts:
///   `texture 1 1 2 assets/logo.png`. A model's line has its two types' names and sizes after the
///   path, whatever build wrote them;
/// - a removal of a texture or model by index and path (`removed texture 2 assets/logo.png`), or
///   of a mesh by index (`removed mesh 3`);
/// - a shader or mesh the engine added when first asked for, by index and name
///   (`engine shader 5 text`, `engine mesh 2 grid`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Again<'a> {
    Load(Asset<&'a str>),
    RemoveTexture(u32, &'a str),
    RemoveModel(usize, &'a str),
    RemoveMesh(u32),
    Engine {
        mesh: bool,
        index: u32,
        name: &'a str,
    },
}

/// What a restored run takes again of `line`. `None` for a mesh's or shader's, which its layers
/// add as they load.
pub(crate) fn again(line: &str) -> Option<Again<'_>> {
    let (kind, rest) = line.split_once(' ')?;
    if kind == "engine" {
        let (kind, rest) = rest.split_once(' ')?;
        let (index, name) = rest.split_once(' ')?;
        let mesh = match kind {
            "shader" => false,
            "mesh" => true,
            _ => return None,
        };
        let index = index.parse().ok()?;
        return Some(Again::Engine { mesh, index, name });
    }
    if kind == "removed" {
        let (kind, rest) = rest.split_once(' ')?;
        let (index, path) = match rest.split_once(' ') {
            Some((index, path)) => (index, Some(path)),
            None => (rest, None),
        };
        return match (kind, path) {
            ("texture", Some(path)) => Some(Again::RemoveTexture(index.parse().ok()?, path)),
            ("model", Some(path)) => Some(Again::RemoveModel(index.parse().ok()?, path)),
            ("mesh", None) => Some(Again::RemoveMesh(index.parse().ok()?)),
            _ => None,
        };
    }
    // Three counts, then the path.
    let path = rest.splitn(4, ' ').nth(3)?;
    let asset = match kind {
        "texture" => Asset::Texture(path),
        "model" => Asset::Model(path.rsplitn(5, ' ').nth(4)?),
        "font" => Asset::Font(path),
        "prefab" => Asset::Prefab(path),
        _ => return None,
    };
    Some(Again::Load(asset))
}

/// Writes the snapshot `text` at `path` whole or not at all, keeping the last one on failure.
///
/// Fails with `snapshot.file` if it can't.
#[cfg(feature = "dev")]
pub(crate) fn save(path: &Path, text: &str) -> Result<()> {
    crate::dirs::write_whole(path, text.as_bytes()).map_err(|error| {
        let message = format!("can't write the snapshot {}", path.display());
        Error::new("snapshot.file", message).with_source(error)
    })
}

/// A snapshot file as a restored run reads it: the checked header, the world's lines (loaded once
/// the layers have) and what its run was doing.
pub(crate) struct Snapshot {
    /// Where it came from: a file's path or a lockstep host.
    pub(crate) from: String,
    text: String,
    layers: Vec<String>,
    ticks: u64,
    hash: WorldHash,
    /// What its run's layers added, in order, and the textures it loaded where text named them.
    assets: Vec<String>,
    /// The name its run gave each texture it named otherwise than by path, keyed by path. Its
    /// values name textures by these.
    #[cfg_attr(
        not(feature = "dev"),
        allow(
            dead_code,
            reason = "builds without `dev` name every texture by its path"
        )
    )]
    pub(crate) names: HashMap<Arc<str>, Arc<str>>,
    /// Scripts: a path from the project's root and the text as last applied.
    pub(crate) scripts: Vec<(String, String)>,
    pub(crate) mods: Vec<ModText>,
    pub(crate) scenes: Vec<SceneFile>,
    /// The scene files' texts its run kept for scene changes.
    pub(crate) cached: Vec<SceneFile>,
    /// `None` for a snapshot written before the open scene was.
    pub(crate) open: Option<Open>,
    /// The indexes of its world's lines.
    world: Range<usize>,
    pub(crate) stopped: bool,
    /// Whether its simulation was paused (lasts across a stop).
    #[cfg_attr(
        not(feature = "dev"),
        allow(dead_code, reason = "only development builds pause")
    )]
    pub(crate) paused: bool,
    /// The world as edited, if taken while a copy of it played.
    pub(crate) held: Option<Held>,
}

/// The world as edited a snapshot holds: ticks, hash, scene files as last applied, open scene (if
/// any) and line indexes.
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
    /// Reads the snapshot file at `path` for app `app`, checking its header.
    ///
    /// Fails with:
    /// - `snapshot.file` if it can't be read;
    /// - `snapshot.format` if it isn't a snapshot of this format, is another app's, or is cut short
    ///   (its last line isn't `end`).
    pub(crate) fn read(path: &Path, app: &str) -> Result<Self> {
        let text = fs::read_to_string(path).map_err(|error| {
            let message = format!("can't read the snapshot {}", path.display());
            Error::new("snapshot.file", message).with_source(error)
        })?;
        Self::parse(text, path.display().to_string(), app)
    }

    /// Reads snapshot `text` from `from` for app `app`, checking its header and what follows its
    /// world: `stopped` then `end`; or `edited`, the world as edited's ticks and hash, its scene
    /// files, open scene and lines, with no `stopped` or other `edited` after; or `end`, for a run
    /// playing from its start. Any takes `paused` before `end` if the run was paused.
    ///
    /// Fails with `snapshot.format` as [`read`](Self::read) does, and for a bad scene file or open
    /// scene line, or anything else after the world.
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
        let mut scripts = Vec::new();
        while let Some(script) = lines
            .get(world)
            .and_then(|line| line.strip_prefix("script "))
        {
            let read = Text::parse(script, "script ".len() + 1)
                .ok()
                .and_then(|text| {
                    let Value::List(strings) = text.value(&World::default()) else {
                        return None;
                    };
                    let [Value::String(path), Value::String(text)] =
                        <[Value; 2]>::try_from(strings).ok()?
                    else {
                        return None;
                    };
                    Some((path, text))
                });
            scripts.push(read.ok_or_else(|| {
                refused(format!(
                    "line {}: a script is a list of its path and its text",
                    world + 1
                ))
            })?);
            world += 1;
        }
        let mut mods = Vec::new();
        while let Some(text) = lines.get(world).and_then(|line| line.strip_prefix("mod ")) {
            mods.push(read_mod(text, "mod ".len() + 1).ok_or_else(|| {
                refused(format!(
                    "line {}: a mod is a list of its manifest's text, then each of its scripts' \
                     path and text",
                    world + 1
                ))
            })?);
            world += 1;
        }
        // The `kind` scene files from line `at` and the next line's index.
        let read_files = |mut at: usize, kind: &str| {
            let mut scenes = Vec::new();
            while let Some(scene) = lines.get(at).and_then(|line| line.strip_prefix(kind)) {
                let read = Text::parse(scene, kind.len() + 1).ok().and_then(|text| {
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
            Ok((scenes, at))
        };
        let (cached, world) = read_files(world, "cached ")?;
        // The scene lines from `at`, the open scene's if any, and the next line's index.
        let read_scenes = |at: usize| {
            let (scenes, mut at) = read_files(at, "scene ")?;
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
        // `paused` (last before `end`) if the run was, else `end`.
        let mut last = lines.len() - 1;
        let paused = last > world && lines[last - 1] == "paused";
        if paused {
            last -= 1;
        }
        // The world's lines end at `end`, `paused`, `stopped` or `edited`, which none starts with.
        let tails =
            |line: &str| line == "stopped" || line == "paused" || line.starts_with("edited ");
        let end = (world..=last)
            .find(|&at| at == last || tails(lines[at]))
            .expect("the last is");
        let (mut stopped, mut held) = (false, None);
        if end != last && lines[end] == "paused" {
            let what = format!("line {}: `paused` comes last, before `end`", end + 1);
            return Err(refused(what));
        }
        if lines[end] == "stopped" {
            if end + 1 != last {
                let what = format!(
                    "line {}: `stopped` is followed by `end` alone, or by `paused` and `end`",
                    end + 1
                );
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
            scripts,
            mods,
            scenes,
            cached,
            open,
            world: world..end,
            stopped,
            paused,
            held,
            text,
        })
    }

    /// The `asset` lines, in order: what its run's layers added and removed, what it loaded where
    /// text named it, and what the engine added when first asked for. A restored run takes them
    /// again before its world, past what its own layers noted alike
    /// ([`Context::load_restored`](crate::Context::load_restored)).
    pub(crate) fn added(&self) -> &[String] {
        &self.assets
    }

    /// Repoints each texture, model and font its run named by ID to its file's current path, where
    /// `catalog` (the restored run's database) holds that ID for another path and the old file is
    /// gone (a rename whose ID file moved). It rewrites the `asset` lines (removals too) and
    /// names, so the run loads the asset from there and its own `asset` line, now the snapshot's,
    /// keeps what's opaque. An asset whose file is still in place keeps its path, even if a copy
    /// took its ID. A model's images keep theirs.
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
            let path = match again(line) {
                Some(Again::Load(
                    Asset::Texture(path)
                    | Asset::Model(path)
                    | Asset::Font(path)
                    | Asset::Prefab(path),
                )) => path,
                Some(Again::RemoveTexture(_, path) | Again::RemoveModel(_, path)) => path,
                Some(Again::RemoveMesh(_) | Again::Engine { .. }) | None => continue,
            };
            let Some(now) = moved.get(path) else {
                continue;
            };
            // The kind and counts precede the path; a model's types follow it.
            let at = path.as_ptr() as usize - line.as_ptr() as usize;
            *line = format!("{}{now}{}", &line[..at], &line[at + path.len()..]);
        }
        let names = self.names.drain().map(|(path, name)| {
            let now = moved.get(&path).cloned().unwrap_or(path);
            (now, name)
        });
        self.names = names.collect();
    }

    /// Loads the snapshot's world into `world`, replacing its entities ([`World::load_whole`]).
    /// The layers (`layers`, bottom up) have registered its component types and added its assets.
    ///
    /// What's opaque comes back if the layers added the snapshot's assets as their first, in order.
    /// A texture its values name by its run's name (as by an ID since changed) comes back by that
    /// name's path; the hash is checked under the run's names, and the run then names it its own
    /// way. Logs each change made to match current types, once. Returns the ticks the simulation
    /// had run, which the run goes on from.
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

    /// Like [`restore`](Self::restore) for the world as edited, into `world` (a copy of the world
    /// as the layers loaded it). Does nothing if the snapshot holds none.
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

    /// Loads the world at `lines` like [`restore`](Self::restore), checking its saved `hash` and
    /// calling it `what` in warnings.
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
        // Its values and hash use its run's texture names, so a run naming them otherwise (after
        // an ID file changed) loads by those, then goes on with its own.
        #[cfg(feature = "dev")]
        let own = {
            let own = Arc::clone(world.shared_assets());
            let alike = own.named_from((0, 0, 0, 0)).count() == self.names.len()
                && (own.named_from((0, 0, 0, 0))).all(|(name, path)| {
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
    use crate::{Context, Layer, MeshDraw, Sprite, Text, Transform};

    /// [`super::write`] for `world`, with no scripts or mods.
    fn write(
        world: &World,
        scenes: &[SceneFile],
        open: &Open,
        run: &Run,
        app: &str,
        layers: &[&str],
        ticks: u64,
    ) -> Result<String> {
        let of = Of {
            world,
            scenes,
            cached: &[],
            open,
            scripts: &[],
            mods: &[],
            seen: None,
        };
        super::write(&of, run, app, layers, ticks)
    }

    fn loads(snapshot: &Snapshot) -> Vec<Asset<&str>> {
        let taken = snapshot.added().iter().filter_map(|line| again(line));
        let loads = taken.filter_map(|again| match again {
            Again::Load(asset) => Some(asset),
            _ => None,
        });
        loads.collect()
    }

    /// `asset` lines give a texture's, model's or font's path (spaces included; a model's with
    /// any types), a removal's index and path (a mesh's, index alone), or an engine's shader's or
    /// mesh's index and name; a mesh's or shader's, nothing.
    #[test]
    fn asset_lines_give_what_a_restore_takes_again() {
        use Again::{Engine, Load, RemoveMesh, RemoveModel, RemoveTexture};
        let model = format!(
            "model 3 4 2 assets/a truck.glb {}",
            crate::layer::model_types()
        );
        let lines = [
            (
                "texture 1 1 2 assets/a stone.png",
                Some(Load(Asset::Texture("assets/a stone.png"))),
            ),
            (
                model.as_str(),
                Some(Load(Asset::Model("assets/a truck.glb"))),
            ),
            (
                "model 3 4 2 m.glb gg::Vertex 48 gg::Params 64",
                Some(Load(Asset::Model("m.glb"))),
            ),
            (
                "font 3 4 5 fonts/a title.ttf",
                Some(Load(Asset::Font("fonts/a title.ttf"))),
            ),
            ("mesh 2 1 1 floor game::Corner 20", None),
            ("shader 1 2 1 shaders/a.slang game::V 12 () 0 Opaque", None),
            (
                "removed texture 2 assets/a stone.png",
                Some(RemoveTexture(2, "assets/a stone.png")),
            ),
            (
                "removed model 0 assets/a truck.glb",
                Some(RemoveModel(0, "assets/a truck.glb")),
            ),
            ("removed mesh 3", Some(RemoveMesh(3))),
            ("removed mesh 3 floor", None),
            ("removed texture two a.png", None),
            ("removed shader 1", None),
            (
                "engine shader 2 model masked",
                Some(Engine {
                    mesh: false,
                    index: 2,
                    name: "model masked",
                }),
            ),
            (
                "engine mesh 1 grid",
                Some(Engine {
                    mesh: true,
                    index: 1,
                    name: "grid",
                }),
            ),
            ("engine shader five text", None),
            ("engine texture 1 white", None),
        ];
        for (line, taken) in lines {
            assert_eq!(again(line), taken, "{line}");
        }
    }

    /// A snapshot read back restores its world, ticks, scripts, mods, scene files and open scene;
    /// one written before the open scene was holds none. Another app's or format's, a cut-short
    /// one, and one with a bad open scene, script or mod line are refused.
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
        // Edited since saved, which the restored run's status says.
        let open = Open {
            author: Some("scenes/grid.ron".to_owned()),
            made: vec![saved.world().id(made).expect("alive")],
            edited: true,
        };
        let scripts = [(
            "scripts/a \"spin\".luau".to_owned(),
            "return gg.declare(\"Spin\", {})\n".to_owned(),
        )];
        let mods = [ModText {
            manifest: "(name: \"a \\\"mod\\\"\")\n".to_owned(),
            scripts: vec![("a.luau".to_owned(), "return 1\n".to_owned())],
        }];
        let of = Of {
            world: saved.world(),
            scenes: &scenes,
            cached: &scenes,
            open: &open,
            scripts: &scripts,
            mods: &mods,
            seen: None,
        };
        let text = super::write(&of, &Run::Playing, "Tested", &["Tester"], 77);
        let text = text.expect("written");
        let thread = std::thread::current().id();
        let dir = Scratch::new(&format!("snapshot-{thread:?}"));
        fs::create_dir_all(&dir.0).expect("a directory");
        let path = dir.0.join("tested.snapshot");
        save(&path, &text).expect("saved");
        let snapshot = Snapshot::read(&path, "Tested").expect("read");
        assert_eq!(
            (&snapshot.scripts[..], &snapshot.scenes[..], &snapshot.open),
            (&scripts[..], &scenes[..], &Some(open))
        );
        assert_eq!(snapshot.mods, mods);
        let mod_line = text.lines().find(|line| line.starts_with("mod "));
        let mod_line = format!("{}\n", mod_line.expect("a mod line"));
        // A script's path without its text.
        let textless = "mod List([String(\"(name: \\\"a\\\")\"), String(\"a.luau\")])\n";
        let script_line = text.lines().find(|line| line.starts_with("script "));
        let script_line = format!("{}\n", script_line.expect("a script line"));
        let pathless = "script List([String(\"scripts/spin.luau\")])\n";
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
            text.replace(
                &open_line,
                "open List([None, List([]), String(\"edited\")])\n",
            ),
            text.replace(&script_line, pathless),
            text.replace(&mod_line, textless),
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

    /// The open scene of a file with a non-Unicode path (left out) is written untitled, owning
    /// what the file's text holds as well as what it made.
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
            edited: false,
        };
        let text = write(&world, &scenes, &open, &Run::Playing, "Tested", &[], 0);
        let snapshot = Snapshot::parse(text.expect("written"), "left".to_owned(), "Tested");
        let snapshot = snapshot.expect("read");
        assert!(snapshot.scenes.is_empty());
        let mut made = vec![a, b];
        made.sort_unstable();
        assert_eq!(
            snapshot.open,
            Some(Open {
                author: None,
                made,
                edited: false
            })
        );
    }

    /// A snapshot taken while a copy of the world as edited played holds that world too, which
    /// comes back at its ticks with its scene files. A stopped one says so and holds the run's
    /// world alone; one playing from its start says neither. A stopped snapshot holding a world as
    /// edited, or with a broken `stopped` or `edited` line, is refused.
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
        // Only the world as edited's is flagged edited; the copy's line is as before the flag.
        let open = |author: &str| Open {
            author: Some(author.to_owned()),
            made: Vec::new(),
            edited: author == "edited",
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

    /// A paused run's snapshot says so, whatever else it was doing; an unpaused one says nothing,
    /// as before pauses were kept.
    #[test]
    fn a_snapshot_keeps_whether_its_run_was_paused() {
        let mut world = World::seeded(1);
        world.spawn(Transform::IDENTITY);
        let none = Open::default();
        let of = Of {
            world: &world,
            scenes: &[],
            cached: &[],
            open: &none,
            scripts: &[],
            mods: &[],
            seen: None,
        };
        let held = Run::Held {
            world: Box::new(world.clone()),
            scenes: Vec::new(),
            open: Open::default(),
            ticks: 3,
        };
        let mut kept = Vec::new();
        for run in [Run::Playing, Run::Stopped, held] {
            for paused in [false, true] {
                let text = write_paused(&of, &run, paused, "Tested", &[], 5).expect("written");
                assert_eq!(text.contains("\npaused\n"), paused);
                let snapshot = Snapshot::parse(text, "paused".to_owned(), "Tested");
                let snapshot = snapshot.expect("read");
                let mut restored = Context::seeded(2);
                snapshot
                    .restore(restored.world_mut(), &[])
                    .expect("restored");
                assert_eq!(restored.world().hash(), world.hash());
                kept.push((snapshot.paused, snapshot.stopped, snapshot.held.is_some()));
            }
        }
        assert_eq!(
            kept,
            [
                (false, false, false),
                (true, false, false),
                (false, true, false),
                (true, true, false),
                (false, false, true),
                (true, false, true),
            ]
        );
        let stopped = write_paused(&of, &Run::Stopped, true, "Tested", &[], 5).expect("written");
        let broken = stopped.replace("stopped\npaused\n", "paused\nstopped\n");
        let refused = Snapshot::parse(broken, "broken".to_owned(), "Tested");
        let refused = refused.err().map(|error| error.code().to_owned());
        assert_eq!(refused.as_deref(), Some("snapshot.format"));
    }

    /// Adds a mesh of `N`-float vertices as it loads.
    struct Drawn<const N: usize>;

    impl<const N: usize> Layer for Drawn<N> {
        fn load(&mut self, cx: &mut Context) -> Result<()> {
            cx.add_mesh("tri", &[[0.0f32; N]; 3], &[0, 1, 2])
                .expect("a mesh");
            Ok(())
        }
    }

    /// What's opaque (a `MeshDraw`) comes back only into a run whose layers added the same assets
    /// in the same order (another vertex type would misread its draws' bytes).
    #[test]
    fn what_is_opaque_comes_back_where_the_same_assets_were_added() {
        let mut saved = Context::seeded(3);
        Drawn::<3>.load(&mut saved).expect("loaded");
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
        Drawn::<3>.load(&mut same).expect("loaded");
        snapshot.restore(same.world_mut(), &[]).expect("restored");
        assert_eq!(same.world().get::<MeshDraw>(drawn), Some(&quad));
        let mut wider = Context::seeded(5);
        Drawn::<4>.load(&mut wider).expect("loaded");
        for mut other in [Context::seeded(5), wider] {
            snapshot.restore(other.world_mut(), &[]).expect("restored");
            assert_eq!(other.world().get::<MeshDraw>(drawn), None);
            assert_eq!(
                other.world().get::<Transform>(drawn),
                Some(&Transform::IDENTITY)
            );
        }
    }

    /// What's opaque comes back though the snapshot's run later loaded a texture where text named
    /// it, removed it, reloaded it and removed a mesh: the restored run takes these again in
    /// order, so its notes are the snapshot's and every index lines up.
    #[test]
    fn what_is_opaque_comes_back_after_a_removal_and_a_reload() {
        let dir = Scratch::new("snapshot-removed");
        let root = dir.0.clone();
        fs::create_dir_all(root.join("assets")).expect("a directory");
        let png =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        fs::write(root.join("assets/stone.png"), png).expect("a PNG");
        let stone = "assets/stone.png";
        // The layers' load.
        let load = |cx: &mut Context| {
            let tri = cx.add_mesh("tri", &[[0.0f32; 3]; 3], &[0, 1, 2]);
            tri.expect("a mesh")
        };
        let mut saved = Context::seeded(3).root(root.clone());
        let tri = load(&mut saved);
        saved.load_named([Asset::Texture(stone)], false);
        let first = saved.world().assets().texture(stone).expect("loaded");
        saved.remove_texture(first);
        saved.load_named([Asset::Texture(stone)], false);
        let anew = saved.world().assets().texture(stone).expect("loaded anew");
        assert_ne!(anew, first);
        saved.remove_mesh(tri);
        let draw = MeshDraw::from_parts(tri.index(), 0, false, false, &[]).expect("a draw");
        let sprite = Sprite {
            texture: Some(anew),
            ..Sprite::default()
        };
        let drawn = saved.world_mut().spawn((Transform::IDENTITY, draw, sprite));
        let none = Open::default();
        let text = write(saved.world(), &[], &none, &Run::Playing, "Tested", &[], 0);
        let text = text.expect("written");
        assert!(
            text.contains(&format!("asset removed mesh {}\n", tri.index())),
            "{text}"
        );
        let snapshot = Snapshot::parse(text, "a test".to_owned(), "Tested").expect("read");

        let mut restored = Context::seeded(4).root(root);
        load(&mut restored);
        restored.load_restored(snapshot.added());
        let notes = restored.world().assets().added().to_vec();
        assert_eq!(notes, saved.world().assets().added(), "every note alike");
        snapshot
            .restore(restored.world_mut(), &[])
            .expect("restored");
        let world = restored.world();
        assert_eq!(world.get::<MeshDraw>(drawn), Some(&draw));
        assert_eq!(world.get::<Sprite>(drawn), Some(&sprite));
        assert_eq!(world.hash(), saved.world().hash());
        assert!(world.assets().removed(first) && !world.assets().removed(anew));
        restored.remove_mesh(tri);
        assert_eq!(
            restored.world().assets().added(),
            notes,
            "removed again already"
        );
    }

    /// Shapes come back under their handles, drawing: one a layer asked for as it attached, which a
    /// restored run's layers don't ask for again, and one text named. Each is made again at its
    /// note, with no file to find.
    #[test]
    fn shapes_come_back_under_their_handles() {
        let mut saved = Context::seeded(3);
        let sphere = saved.shape(crate::Shape::Sphere);
        saved.load_named([Asset::Model("gg:cube")], false);
        let cube = saved.world().assets().model("gg:cube").expect("loaded");
        let models = [sphere, cube];
        let world = saved.world_mut();
        let roots = models.map(|model| model.spawn(world, Transform::IDENTITY));
        let none = Open::default();
        let text = write(saved.world(), &[], &none, &Run::Playing, "Tested", &[], 0);
        let snapshot = Snapshot::parse(text.expect("written"), "a test".to_owned(), "Tested");
        let snapshot = snapshot.expect("read");

        let mut restored = Context::seeded(4);
        restored.load_restored(snapshot.added());
        let notes = restored.world().assets().added();
        assert_eq!(notes, saved.world().assets().added(), "every note alike");
        snapshot
            .restore(restored.world_mut(), &[])
            .expect("restored");
        let world = restored.world();
        for (root, model) in roots.into_iter().zip(models) {
            let root_model = world.get::<crate::ModelRoot>(root);
            assert_eq!(root_model, Some(&crate::ModelRoot(model)));
            let mut nodes = world.children(root).expect("alive");
            let node = nodes.next().expect("a node");
            assert!(world.get::<MeshDraw>(node).is_some(), "drawing");
        }
        assert_eq!(world.hash(), saved.world().hash());
    }

    /// What's opaque comes back though the snapshot's run drew a circle between two loads, which
    /// added the engine's circle shader: the restored run adds it again at its note, so the
    /// texture's note, and every index after it, are the snapshot's.
    #[test]
    fn what_is_opaque_comes_back_after_a_frame_drew_between_loads() {
        let dir = Scratch::new("snapshot-drew");
        let root = dir.0.clone();
        fs::create_dir_all(root.join("assets")).expect("a directory");
        let png =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        fs::write(root.join("assets/stone.png"), png).expect("a PNG");
        // The layers' load.
        let load = |cx: &mut Context| {
            let tri = cx.add_mesh("tri", &[[0.0f32; 3]; 3], &[0, 1, 2]);
            tri.expect("a mesh")
        };
        let mut saved = Context::seeded(3).root(root.clone());
        let tri = load(&mut saved);
        saved.draw_circle(crate::math::Mat4::IDENTITY, crate::Circle::default());
        saved.load_named([Asset::Texture("assets/stone.png")], false);
        let draw = MeshDraw::from_parts(tri.index(), 0, false, false, &[]).expect("a draw");
        let drawn = saved.world_mut().spawn((Transform::IDENTITY, draw));
        let none = Open::default();
        let text = write(saved.world(), &[], &none, &Run::Playing, "Tested", &[], 0);
        let snapshot = Snapshot::parse(text.expect("written"), "a test".to_owned(), "Tested");
        let snapshot = snapshot.expect("read");

        let mut restored = Context::seeded(4).root(root);
        load(&mut restored);
        restored.load_restored(snapshot.added());
        let notes = restored.world().assets().added();
        assert_eq!(notes, saved.world().assets().added(), "every note alike");
        snapshot
            .restore(restored.world_mut(), &[])
            .expect("restored");
        assert_eq!(restored.world().get::<MeshDraw>(drawn), Some(&draw));
    }

    /// A removal is taken again only while the restored run's notes match the snapshot's up to it,
    /// so its index is the asset that run removed. Newer layers that add the texture at that index
    /// and path, then one more mesh (notes part before the removal), keep it.
    #[test]
    fn a_removal_is_taken_again_only_where_the_notes_line_up() {
        let dir = Scratch::new("snapshot-parted");
        let root = dir.0.clone();
        fs::create_dir_all(root.join("assets")).expect("a directory");
        let png =
            crate::image::tests::png(png::ColorType::Rgba, png::BitDepth::Eight, 1, &[9; 4], None);
        let path = root.join("assets/stone.png");
        fs::write(&path, png).expect("a PNG");
        let stone = "assets/stone.png";
        let corners = [[0.0f32; 3]; 3];
        let mut saved = Context::seeded(3).root(root.clone());
        saved.add_mesh("tri", &corners, &[0, 1, 2]).expect("a mesh");
        saved.load_named([Asset::Texture(stone)], false);
        let loaded = saved.world().assets().texture(stone).expect("loaded");
        saved.remove_texture(loaded);
        let none = Open::default();
        let text = write(saved.world(), &[], &none, &Run::Playing, "Tested", &[], 0);
        let snapshot = Snapshot::parse(text.expect("written"), "a test".to_owned(), "Tested");
        let snapshot = snapshot.expect("read");

        let mut newer = Context::seeded(4).root(root);
        newer.add_mesh("tri", &corners, &[0, 1, 2]).expect("a mesh");
        let own = newer.add_texture("stone", &path).expect("a texture");
        assert_eq!(own, loaded, "at the index the removal gives, from its path");
        newer
            .add_mesh("more", &corners, &[0, 1, 2])
            .expect("a mesh");
        newer.load_restored(snapshot.added());
        let assets = newer.world().assets();
        assert!(!assets.removed(own), "its own texture kept");
        let removals = assets
            .added()
            .iter()
            .filter(|note| note.starts_with("removed "));
        assert_eq!(removals.count(), 0);
    }

    /// A run whose ID files since gave a texture another ID (as after a checkout) still restores
    /// each texture the snapshot names by ID, hashing as saved under the snapshot's names, so no
    /// change is warned of. It then goes on by its own name, and so hashes otherwise.
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

    /// A texture loaded where text named it (not by a layer) is loaded again from its `asset`
    /// line's path; if its file is gone, a stand-in named as the snapshot names it takes its
    /// place. Either way the world hashes as saved, with no change warned of. A run that follows
    /// another finds such a line's ID file (a stand-in's too) by its own database.
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
        saved.load_named([Asset::Texture(id)], false);
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
        assert_eq!(loads(&snapshot), [Asset::Texture("assets/stone.png")]);
        // Following another run, with no name for the file, it finds the ID by its own database.
        let mut following = Context::seeded(3).root(root.clone());
        following.name_as([]);
        following.load_named([Asset::Texture(id)], true);
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

    /// A font loaded where text named it, by ID, is loaded again from its `asset` line's path, so
    /// a text in it comes back in it, with no change warned of.
    #[test]
    fn a_font_loaded_where_named_comes_back() {
        let logged = crate::testing::logged();
        let dir = Scratch::new("snapshot-font");
        let root = dir.0.clone();
        fs::create_dir_all(root.join("fonts")).expect("a directory");
        fs::write(root.join("fonts/title.ttf"), crate::font::INTER).expect("a font");
        let id = "6f708192a3b4c5d6";
        let meta = gg_api::asset::Meta {
            id: id.parse().expect("an ID"),
        };
        fs::write(root.join("fonts/title.ttf.meta"), meta.text()).expect("its ID file");
        let mut saved = Context::seeded(3).root(root.clone());
        saved.load_named([Asset::Font(id)], false);
        let title = saved.world().assets().font(id).expect("loaded");
        let text = Text {
            font: Some(title),
            ..Text::default()
        };
        let shown = saved.world_mut().spawn((Transform::IDENTITY, text));
        let none = Open::default();
        let text = write(saved.world(), &[], &none, &Run::Playing, "Tested", &[], 0);
        let snapshot = Snapshot::parse(text.expect("written"), "a test".to_owned(), "Tested");
        let snapshot = snapshot.expect("read");
        assert_eq!(loads(&snapshot), [Asset::Font("fonts/title.ttf")]);
        let mut restored = Context::seeded(3).root(root);
        let _ = logged.codes.try_iter().count();
        let restoring = Stack::default().restore(&mut restored, &snapshot, false);
        restoring.expect("restored");
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert!(
            !codes.iter().any(|code| code.starts_with("snapshot.")),
            "{codes:?}"
        );
        let world = restored.world();
        let font = world.get::<Text>(shown).and_then(|text| text.font);
        assert_eq!(font.map(|font| world.assets().font_name(font)), Some(id));
    }

    /// A texture loaded where named, its file since renamed with its ID file (as the watcher
    /// moves it), comes back from the new place once the snapshot follows it, with no change
    /// warned of; its `asset` line follows too. One whose file is still in place, a copy having
    /// taken its ID, keeps its path.
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
        saved.load_named([Asset::Texture(id)], false);
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
        // A copy with its ID file, which a first scan gives the ID (first by path), while the
        // texture's file stays put: no rename.
        for name in ["stone.png", "stone.png.meta"] {
            fs::copy(assets.join(name), assets.join(format!("a_{name}"))).expect("copied");
        }
        let copied = Context::seeded(3).root(root.clone());
        let mut kept = parsed();
        kept.follow(copied.catalog());
        assert_eq!(loads(&kept), [Asset::Texture("assets/stone.png")]);
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
        assert_eq!(loads(&snapshot), [Asset::Texture("assets/pillar.png")]);
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
