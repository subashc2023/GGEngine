//! Snapshot files: a run's world saved whole as text, which a run given `--restore` starts from, in
//! another process, after a crash or a rebuild, so that it goes on where the world was: its app,
//! its engine and build, its layers from the bottom up, the ticks the simulation had run, the
//! world's hash, the assets the layers had added, which name what's opaque in the world, the scene
//! files the layers had loaded, each its path, its author and its text as last applied, as a
//! recording's values write them, which the restored run watches, and the world, as
//! [`World::save_whole`] writes it, then `end`.
//!
//! ```text
//! gg-snapshot 1
//! app Sandbox
//! engine 0.1.0 debug
//! layers Sandbox PerfPanel Inspector
//! ticks 1234
//! hash 58183b6352a09a44 fb695ce29daa8479
//! asset texture 1 1 2 assets/checkerboard.png
//! scene List([String("/game/scenes/grid.ron"), String("scenes/grid.ron"), String("(entities: ...")])
//! stream 3f8a6c2e9b1d4057
//! slots 10530
//! free 12 7
//! table MapTile Sprite Transform
//! row 17 5f3a9c0e12b47d68 List([Struct([]), Struct([("color", Vec4(1.0, 1.0, 1.0, 1.0)), ...
//! children 5f3a9c0e12b47d68 1a2b3c4d5e6f7081
//! resource Physics2D (gravity:(0.0,-9.81),rapier:(...),linked:[],children:[])
//! end
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use gg_core::{Error, Result};
use tracing::{info, warn};

use crate::recording::text::Text;
use crate::reflect::Value;
use crate::scene_files::SceneFile;
use crate::{World, WorldHash};

/// A snapshot file's first line: its format, and the format's version, which a change to its lines
/// or to its values' text that old files can't be read by takes to the next.
const FORMAT: &str = "gg-snapshot 1";

/// The text of the snapshot file of `world`, app `app`'s, whose layers are `layers`, from the bottom
/// up, as the simulation had run `ticks` ticks.
///
/// Fails as [`World::save_whole`] does.
#[cfg(feature = "dev")]
pub(crate) fn write(
    world: &World,
    scenes: &[SceneFile],
    app: &str,
    layers: &[&str],
    ticks: u64,
) -> Result<String> {
    use std::fmt::Write as _;

    use crate::recording::{engine, text};

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
    // One whose path isn't Unicode isn't watched after a restore.
    for scene in scenes {
        if let Some(path) = scene.path.to_str() {
            let strings = [path, &scene.author, &scene.text];
            let strings = strings.map(|string| Value::String(string.to_owned()));
            out.push_str("scene ");
            text::write(&mut out, &Value::List(strings.into()), world);
            out.push('\n');
        }
    }
    world.save_whole(&mut out)?;
    out.push_str("end\n");
    Ok(out)
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

/// A snapshot file, as a run restored from it reads it as it starts: its header, checked, and the
/// lines of its world, which load once the layers have.
pub(crate) struct Snapshot {
    path: PathBuf,
    text: String,
    layers: Vec<String>,
    ticks: u64,
    hash: WorldHash,
    /// What the layers of the run it was taken from had added, in order.
    assets: Vec<String>,
    /// The scene files that run's layers had loaded, as last applied.
    pub(crate) scenes: Vec<SceneFile>,
    /// The index of its world's first line.
    world: usize,
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
        let refused =
            |what: String| Error::new("snapshot.format", format!("{}: {what}", path.display()));
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
        let parsed = hash.split_once(' ').and_then(|(content, layout)| {
            let hex = |text: &str| u64::from_str_radix(text, 16).ok();
            Some(WorldHash {
                content: hex(content)?,
                layout: hex(layout)?,
            })
        });
        let hash = parsed.ok_or_else(|| refused(format!("`{hash}` isn't a world's hash")))?;
        let mut world = 6;
        let mut assets = Vec::new();
        while let Some(added) = lines
            .get(world)
            .and_then(|line| line.strip_prefix("asset "))
        {
            assets.push(added.to_owned());
            world += 1;
        }
        let mut scenes = Vec::new();
        while let Some(scene) = lines
            .get(world)
            .and_then(|line| line.strip_prefix("scene "))
        {
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
                    world + 1
                ))
            })?;
            scenes.push(scene);
            world += 1;
        }
        Ok(Self {
            path: path.to_owned(),
            layers: layers.collect(),
            ticks,
            hash,
            assets,
            scenes,
            world,
            text,
        })
    }

    /// Loads the snapshot's world into `world`, whose component types and assets the layers,
    /// `layers` from the bottom up, have registered and added as they loaded, in place of its
    /// entities, as [`World::load_whole`] does; what's opaque in it comes back if the layers added
    /// the assets the snapshot's had, in the same order, as their first. Logs what it changed of
    /// the world to match the types as they are, each change once, and returns the ticks the
    /// simulation had run, which the run goes on from.
    ///
    /// Fails as `load_whole` does.
    pub(crate) fn restore(&self, world: &mut World, layers: &[&str]) -> Result<u64> {
        let path = self.path.display();
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
        let added = world.assets().added();
        let same = self.assets.len() <= added.len()
            && (self.assets.iter())
                .zip(added)
                .all(|(theirs, ours)| *theirs == **ours);
        let lines = self.text.lines().skip(self.world);
        let lines = (self.world + 1..)
            .zip(lines)
            .take_while(|&(_, line)| line != "end");
        let changes = world.load_whole(lines, same).map_err(|error| {
            let message = format!("{path}: {error:#}");
            Error::new(error.code(), message)
        })?;
        for (change, count) in changes.iter() {
            warn!(
                code = "snapshot.changed",
                path = %path,
                count,
                "The world is restored otherwise than it was saved, to match its types as they \
                 are: {change}"
            );
        }
        let hash = world.hash();
        if changes.is_empty() && hash != self.hash {
            warn!(
                code = "snapshot.hash",
                path = %path,
                "The world restored hashes {hash}, where the world saved hashed {}: it goes on \
                 otherwise than it would have",
                self.hash
            );
        }
        let entities = world.entities(None).map_or(0, |entities| entities.len());
        info!(path = %path, ticks = self.ticks, entities, "Restored the world from a snapshot");
        Ok(self.ticks)
    }
}

// They write snapshots, which only development builds do.
#[cfg(all(test, feature = "dev"))]
mod tests {
    use super::*;
    use crate::dirs::tests::Scratch;
    use crate::{Context, Layer, MeshDraw, Sprite, Transform};

    /// A snapshot written and read back restores the world it was taken of, at its ticks, into a
    /// world its layers loaded; one of another app, of another format or cut short is refused.
    #[test]
    fn a_snapshot_restores_the_world_it_was_taken_of() {
        let mut saved = Context::seeded(1);
        let world = saved.world_mut();
        world.spawn((Transform::IDENTITY, Sprite::default()));
        // A path and a text that need escapes.
        let scene = SceneFile::new(
            "scenes/a \"grid\".ron".into(),
            "scenes/grid.ron".to_owned(),
            "(entities: {\n})\n".to_owned(),
        );
        let scenes = [scene];
        let text = write(saved.world(), &scenes, "Tested", &["Tester"], 77).expect("written");
        let thread = std::thread::current().id();
        let dir = Scratch::new(&format!("snapshot-{thread:?}"));
        fs::create_dir_all(&dir.0).expect("a directory");
        let path = dir.0.join("tested.snapshot");
        save(&path, &text).expect("saved");
        let snapshot = Snapshot::read(&path, "Tested").expect("read");
        assert_eq!(snapshot.scenes, scenes);
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
        let text = write(saved.world(), &[], "Tested", &[], 0).expect("written");
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
}
