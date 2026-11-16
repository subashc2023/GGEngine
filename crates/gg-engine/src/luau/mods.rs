//! Mods: scripts a player installs, as [`Mods`] says.

use std::cell::RefCell;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::rc::{Rc, Weak};

use gg_core::{Error, Result};
use serde::Deserialize;
use tracing::{info, warn};

use super::{Held, Host};
use crate::grant::Grant;
use crate::{Context, World};

/// The most memory a mod's VM may hold; past it an allocation fails as an error.
const MEMORY: usize = 64 << 20;

/// A layer's mods: scripts a player installs. They run like a game's own
/// [`Scripts`](super::Scripts), each mod in a VM of its own. docs/LUAU.md's Mods and The sandbox
/// sections are the reference.
///
/// A mod is a folder holding `mod.ron` and the scripts it lists, as source:
///
/// ```ron
/// (
///     name: "rainbow",
///     version: "1.0.0",
///     game: "Shards",
///     capabilities: ["read:Shard", "write:Sprite"],
///     scripts: ["rainbow.luau"],
/// )
/// ```
///
/// A windowed run's installed mods are those in `mods` in the game's per-user folder. A run given
/// `--mods <DIR>` has that folder's instead; a replay runs its recording's. [`load`](Self::load)
/// loads them in name order, warning of each it refuses; the game runs without it.
/// [`run`](Self::run) runs them where the layer's tick chooses.
///
/// A mod asks for `read` and `write`, of the whole world or of named component types
/// (`read:Shard`), and its scripts are kept to them: `gg.component` refuses a type they may
/// neither read nor write (`mod.ungranted`), and a field access fails unless allowed. Types a mod
/// declares are its own; a name another script declared is refused. A mod reaches the game's
/// entities through a component of its own riding on one of the game's (`gg.declare`'s `on`).
///
/// A game registers its component types before loading mods, since a mod may declare any free
/// name. Once a world has loaded mods, whatever is installed, it refuses a Rust type's
/// registration (`ecs.registered_late`), and a spawn of an unregistered type panics.
///
/// Each mod's VM holds at most 64 MiB, failing at the same allocation on every platform for runs
/// that went alike. Its calls in a tick share one budget of a million Luau interrupts, past which
/// they fail until the next tick. It prints at most 32 lines a second of ticks; `mod.unprinted`
/// warns of the rest.
///
/// A run's mods go whole into its snapshots and recording, so a lockstep joiner runs its host's
/// and a replay its recording's, each granted no more than its own game grants. A run restored
/// from a snapshot loads those installed.
pub struct Mods {
    set: Rc<RefCell<Set>>,
}

/// A mod as it travels in a snapshot or a recording: its manifest's text, and its scripts' (path in
/// its folder, text) in manifest order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModText {
    pub(crate) manifest: String,
    pub(crate) scripts: Vec<(String, String)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    name: String,
    version: String,
    game: String,
    capabilities: Vec<String>,
    scripts: Vec<String>,
}

/// What a [`Mods`] holds: the game's name, the most it grants any mod, and the loaded mods in name
/// order.
struct Set {
    game: String,
    limits: Grant,
    mods: Vec<Loaded>,
}

struct Loaded {
    text: ModText,
    name: String,
    host: Host,
}

/// A run's mods, kept in its context: where they come from, and the [`Mods`] its layers loaded,
/// which a lockstep joiner's [`take`](Self::take) reaches.
#[derive(Default)]
pub(crate) struct RunMods {
    /// The game's name, which a mod's manifest must name.
    pub(crate) game: String,
    /// The folder of the run's installed mods, if it reads any.
    pub(crate) dir: Option<PathBuf>,
    /// A replay's recorded mods, run in place of those installed.
    pub(crate) recorded: Option<Vec<ModText>>,
    sets: Vec<Weak<RefCell<Set>>>,
}

impl Mods {
    /// Loads the run's mods into `cx`'s world, where their scripts declare their components. Each
    /// is granted what it asks for if `limits` allows. Warns of each it refuses:
    ///
    /// - `mod.unreadable`: files that can't be read.
    /// - `mod.bad`: a manifest that doesn't read, a script path that isn't a `.luau` file inside
    ///   its folder, a capability of the tool API's, or a name another mod has.
    /// - `grant.unknown`: a capability that doesn't exist.
    /// - `mod.wrong_game`: another game's mod.
    /// - `mod.ungranted`: more than `limits`, or a chunk reaching past what it asked for.
    /// - `reflect.name_taken`: a declared component type another script declared.
    /// - A script's load failing.
    pub fn load(cx: &mut Context, limits: Grant) -> Self {
        let texts = match &cx.mods.recorded {
            Some(recorded) => recorded.clone(),
            None => cx.mods.dir.as_deref().map(installed).unwrap_or_default(),
        };
        let mut set = Set {
            game: cx.mods.game.clone(),
            limits,
            mods: Vec::new(),
        };
        set.load(cx.world_mut(), texts);
        let set = Rc::new(RefCell::new(set));
        cx.mods.sets.retain(|kept| kept.strong_count() > 0);
        cx.mods.sets.push(Rc::downgrade(&set));
        Self { set }
    }

    /// Runs each mod's scripts like [`Scripts::run`](super::Scripts::run), the mods in name order.
    pub fn run(&mut self, world: &mut World) {
        for loaded in &mut self.set.borrow_mut().mods {
            loaded.host.run(world);
        }
    }
}

impl Set {
    /// Loads `texts`' mods in name order, as [`Mods::load`] says.
    fn load(&mut self, world: &mut World, texts: Vec<ModText>) {
        world.mods_loaded();
        let mut read = Vec::new();
        for text in texts {
            match manifest(&text.manifest) {
                Ok(manifest) => read.push((manifest, text)),
                Err(error) => warn!(code = error.code(), "{error:#}; the game runs without it"),
            }
        }
        read.sort_by(|(a, _), (b, _)| a.name.cmp(&b.name));
        for (manifest, text) in read {
            match self.load_one(world, &manifest, text) {
                Ok(loaded) => {
                    let grant = loaded.host.shared.held.as_ref().map(|held| &held.grant);
                    info!(
                        "mod" = manifest.name,
                        version = manifest.version,
                        grant = %grant.expect("a mod's host holds its grant"),
                        "Mod loaded"
                    );
                    self.mods.push(loaded);
                }
                Err(error) => warn!(
                    code = error.code(),
                    "mod" = manifest.name,
                    "{error:#}; the game runs without the mod"
                ),
            }
        }
    }

    /// Loads the mod `text` holds, whose manifest is `manifest`.
    fn load_one(&self, world: &mut World, manifest: &Manifest, text: ModText) -> Result<Loaded> {
        let name = &manifest.name;
        let bad = |what: String| Error::new("mod.bad", format!("mod `{name}`: {what}"));
        if self.mods.iter().any(|loaded| loaded.name == *name) {
            return Err(bad("another mod of its name has loaded".to_owned()));
        }
        if manifest.game != self.game {
            return Err(Error::new(
                "mod.wrong_game",
                format!(
                    "mod `{name}` is for `{}`, not `{}`",
                    manifest.game, self.game
                ),
            ));
        }
        let grant = Grant::parse(&manifest.capabilities)?;
        if let Some(capability) = grant.iter().find(|capability| !capability.of_the_world()) {
            return Err(bad(format!(
                "it asks for `{capability}`, the tool API's: a mod asks for `read` and `write`"
            )));
        }
        let beyond: Vec<String> = grant
            .beyond(&self.limits)
            .map(ToString::to_string)
            .collect();
        if !beyond.is_empty() {
            return Err(Error::new(
                "mod.ungranted",
                format!(
                    "mod `{name}` asks for `{}`, more than the game grants a mod, `{}`",
                    beyond.join("`, `"),
                    self.limits
                ),
            ));
        }
        let listed = text.scripts.iter().map(|(path, _)| path);
        if !listed.eq(&manifest.scripts) {
            return Err(bad("its scripts aren't those its manifest lists".to_owned()));
        }
        if let Some(path) = manifest.scripts.iter().find(|path| !inside(path)) {
            return Err(bad(format!(
                "its script `{path}` isn't a `.luau` file's path inside its folder"
            )));
        }
        let mut host = Host::new(Some((Held::new(name.clone(), grant), MEMORY)))?;
        for (path, script) in &text.scripts {
            host.load(world, &format!("{name}/{path}"), script.clone())?;
        }
        Ok(Loaded {
            text,
            name: name.clone(),
            host,
        })
    }
}

impl RunMods {
    /// The mods the run's layers loaded, as they came, without duplicates.
    pub(crate) fn applied(&self) -> Vec<ModText> {
        let mut applied: Vec<ModText> = Vec::new();
        for set in self.sets.iter().filter_map(Weak::upgrade) {
            for loaded in &set.borrow().mods {
                if !applied.contains(&loaded.text) {
                    applied.push(loaded.text.clone());
                }
            }
        }
        applied
    }

    /// Has each of the run's [`Mods`] run `mods` in place of those it loaded, unless they're the
    /// same. A lockstep joiner's: its host's mods come with its world.
    pub(crate) fn take(&self, world: &mut World, mods: &[ModText]) {
        for set in self.sets.iter().filter_map(Weak::upgrade) {
            let mut set = set.borrow_mut();
            if set.mods.iter().map(|loaded| &loaded.text).eq(mods) {
                continue;
            }
            set.mods.clear();
            set.load(world, mods.to_vec());
        }
    }
}

/// Reads the manifest in `text`, a `mod.ron`'s.
///
/// Fails with `mod.bad` if it isn't one.
fn manifest(text: &str) -> Result<Manifest> {
    ron::from_str(text).map_err(|error| {
        Error::new(
            "mod.bad",
            format!("a mod's `mod.ron` doesn't read as one: {error}"),
        )
    })
}

/// Whether `path`, a script's in a manifest, is a `.luau` file inside the mod's folder: relative,
/// `/`-separated, so every platform reads it alike.
fn inside(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && !path.to_str().is_some_and(|path| path.contains('\\'))
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && path
            .extension()
            .is_some_and(|extension| extension == "luau")
}

/// The mods installed in `dir`, one folder each, in folder-name order; none if `dir` isn't there.
///
/// Warns `mod.unreadable` of a mod whose files can't be read, and `mod.bad` of one whose manifest
/// doesn't read or names a script outside its folder.
fn installed(dir: &Path) -> Vec<ModText> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut folders: Vec<PathBuf> = (entries.filter_map(|entry| Some(entry.ok()?.path())))
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    let read = |folder: &Path| -> Result<ModText> {
        let unreadable = |path: &Path, error: std::io::Error| {
            let what = format!("can't read {}", path.display());
            Error::new("mod.unreadable", what).with_source(error)
        };
        let at = folder.join("mod.ron");
        let text = fs::read_to_string(&at).map_err(|error| unreadable(&at, error))?;
        let listed = manifest(&text).map_err(|error| {
            let message = format!("{}: {error}", at.display());
            error.with_message(message)
        })?;
        let mut scripts = Vec::new();
        for path in listed.scripts {
            if !inside(&path) {
                let what = format!(
                    "{}: its script `{path}` isn't a `.luau` file's path inside its folder",
                    at.display()
                );
                return Err(Error::new("mod.bad", what));
            }
            let file = folder.join(&path);
            let script = fs::read_to_string(&file).map_err(|error| unreadable(&file, error))?;
            scripts.push((path, script));
        }
        Ok(ModText {
            manifest: text,
            scripts,
        })
    };
    let mut mods = Vec::new();
    for folder in folders {
        match read(&folder) {
            Ok(text) => mods.push(text),
            Err(error) => warn!(code = error.code(), "{error:#}; the game runs without it"),
        }
    }
    mods
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Component;
    use crate::dirs::tests::Scratch;
    use crate::reflect::Value;

    /// What the mods' scripts reach: a count, and a mark only a whole-world write grant reaches.
    #[derive(Clone, Component)]
    struct Counter {
        count: u32,
        mark: u32,
    }

    /// Counts each counter, riding on it.
    const COUNTING: &str = "local Counter = gg.component(\"Counter\")\n\
        local Counting = gg.declare(\"Counting\", { by = 1 }, { on = \"Counter\" })\n\
        function Counting.tick(entity, dt)\n\
            entity[Counter.count] = entity[Counter.count] + entity[Counting.by]\n\
        end\n\
        return Counting\n";

    /// Writes mod `name` of `script` in `dir`, for `game`, asking for `capabilities`.
    fn install(dir: &Path, name: &str, game: &str, capabilities: &str, script: &str) {
        let folder = dir.join(name);
        fs::create_dir_all(&folder).expect("a directory");
        let manifest = format!(
            "(name: \"{name}\", version: \"1.0.0\", game: \"{game}\", \
             capabilities: [{capabilities}], scripts: [\"{name}.luau\"])"
        );
        fs::write(folder.join("mod.ron"), manifest).expect("written");
        fs::write(folder.join(format!("{name}.luau")), script).expect("written");
    }

    /// A context of game `Modded` with mods installed in `dir`, `Counter` registered and `counters`
    /// spawned.
    fn modded(dir: &Path, counters: usize) -> (Context, Vec<crate::Entity>) {
        let mut cx = Context::seeded(1);
        cx.mods.game = "Modded".to_owned();
        cx.mods.dir = Some(dir.to_owned());
        let world = cx.world_mut();
        world.register::<Counter>().expect("described");
        let counters = (0..counters).map(|_| world.spawn(Counter { count: 0, mark: 0 }));
        let counters = counters.collect();
        (cx, counters)
    }

    /// The names of the mods that loaded.
    fn names(cx: &Context) -> Vec<String> {
        let applied = cx.mods.applied();
        applied
            .iter()
            .map(|text| manifest(&text.manifest).expect("read").name)
            .collect()
    }

    /// Mods load in name order, each held to what it asks for and the game grants. Refused:
    /// asking for more (`mod.ungranted`), another game's, a tool API capability, a chunk reaching
    /// an ungranted type, and another mod's component type. A tick writing what it isn't granted
    /// fails there with a warning, and the game goes on.
    #[test]
    fn mods_load_held_to_what_they_ask_for_and_the_game_grants() {
        let logged = crate::testing::logged();
        let dir = Scratch::new("mods-grants");
        let both = "\"read:Counter\", \"write:Counter\"";
        install(&dir.0, "b-counting", "Modded", both, COUNTING);
        // Another mod's component type, which it would take as its own.
        install(&dir.0, "c-taking", "Modded", both, COUNTING);
        let marking = COUNTING
            .replace("Counting", "Marking")
            .replace("Counter.count", "Counter.mark");
        install(&dir.0, "a-marking", "Modded", "\"read\"", &marking);
        install(&dir.0, "greedy", "Modded", "\"write\"", COUNTING);
        install(&dir.0, "other", "Other", both, COUNTING);
        install(&dir.0, "viewer", "Modded", "\"view\"", COUNTING);
        let reaching = "local Counter = gg.component(\"Counter\")\nreturn gg.declare(\"R\", {})";
        install(&dir.0, "reaching", "Modded", "\"read:Nope\"", reaching);
        let (mut cx, counters) = modded(&dir.0, 2);
        let limits = Grant::parse(["read", "write:Counter"]).expect("a grant");
        let mut mods = Mods::load(&mut cx, limits);
        assert_eq!(names(&cx), ["a-marking", "b-counting"]);
        mods.run(cx.world_mut());
        mods.run(cx.world_mut());
        for counter in counters {
            let counted = cx.world().get::<Counter>(counter).expect("a counter");
            assert_eq!((counted.count, counted.mark), (2, 0));
        }
        let codes: Vec<String> = logged
            .codes
            .try_iter()
            .filter(|code| !code.is_empty())
            .collect();
        assert_eq!(
            codes,
            [
                "reflect.name_taken",
                "mod.ungranted",
                "mod.wrong_game",
                "mod.ungranted",
                "mod.bad",
                "script.error"
            ]
        );
    }

    /// A mod's calls in a tick share one budget until the next tick; prints past 32 a second of
    /// ticks go unprinted; allocations past its memory fail, and the next tick goes on. The memory
    /// its VM holds after loading a script that allocates is alike on every platform.
    #[test]
    fn a_mod_is_held_to_its_budget_its_prints_and_its_memory() {
        let dir = Scratch::new("mods-limits");
        let busy = "local Counter = gg.component(\"Counter\")\n\
            local Busy = gg.declare(\"Busy\", { turns = 400000, megabytes = 0 },\n\
                { on = \"Counter\" })\n\
            local kept = {}\n\
            for i = 1, 1000 do kept[i] = tostring(i) end\n\
            function Busy.tick(entity, dt)\n\
                for i = 1, entity[Busy.turns] do end\n\
                for i = 1, 40 do print(i) end\n\
                local held = {}\n\
                for i = 1, entity[Busy.megabytes] do held[i] = string.rep(\"x\", 1048576) end\n\
                entity[Counter.count] = entity[Counter.count] + 1\n\
                local _ = kept\n\
            end\n\
            return Busy\n";
        install(
            &dir.0,
            "busy",
            "Modded",
            "\"read:Counter\", \"write:Counter\"",
            busy,
        );
        let (mut cx, counters) = modded(&dir.0, 3);
        let counters: [crate::Entity; 3] = counters.try_into().expect("three");
        let mut mods = Mods::load(&mut cx, Grant::all());
        let held = |mods: &Mods| {
            let set = mods.set.borrow();
            let host = &set.mods[0].host;
            let held = host.shared.held.as_ref().expect("a mod's");
            (held.unprinted.get(), host.lua.used_memory())
        };
        // A reference: a Luau or mlua change may move it (object sizes, a VM's startup), a
        // platform never.
        assert_eq!(
            held(&mods).1,
            509_288,
            "the memory the mod's VM holds as it has loaded"
        );
        let counts = |cx: &Context| {
            counters.map(|counter| cx.world().get::<Counter>(counter).expect("a counter").count)
        };
        mods.run(cx.world_mut());
        assert_eq!(counts(&cx), [1, 1, 0], "the third past the tick's budget");
        // Each printed 40 of the 32 a second of ticks has.
        assert_eq!(held(&mods).0, 40 + 40 - 32);
        mods.run(cx.world_mut());
        assert_eq!(counts(&cx), [2, 2, 0]);
        let world = cx.world_mut();
        for counter in counters {
            world
                .set_field(counter, "Busy", "turns", &Value::F64(0.0))
                .expect("set");
        }
        let much = Value::F64(80.0);
        world
            .set_field(counters[0], "Busy", "megabytes", &much)
            .expect("set");
        mods.run(cx.world_mut());
        assert_eq!(counts(&cx), [2, 3, 1], "the first past its memory");
        assert!(held(&mods).1 <= MEMORY, "{}", held(&mods).1);
        cx.world_mut()
            .set_field(counters[0], "Busy", "megabytes", &Value::F64(8.0))
            .expect("set");
        mods.run(cx.world_mut());
        assert_eq!(counts(&cx), [3, 4, 2]);
    }

    /// Once a world has loaded mods, registering a Rust type or inserting an unregistered one is
    /// refused, whether or not a mod declared its name, in the world and its copies. Types
    /// registered before go on as ever.
    #[test]
    fn a_type_that_registers_after_mods_load_is_refused_whatever_is_installed() {
        /// A type the game registers late; a mod declares its name.
        #[derive(Clone, Component)]
        struct Late {
            at: u32,
        }
        let dir = Scratch::new("mods-late");
        for installed in [false, true] {
            if installed {
                let taking = "return gg.declare(\"Late\", { at = 1 })";
                install(&dir.0, "taking", "Modded", "\"read\"", taking);
            }
            let (mut cx, counters) = modded(&dir.0, 1);
            let before = cx.world().clone();
            let _mods = Mods::load(&mut cx, Grant::all());
            assert_eq!(names(&cx).len(), usize::from(installed));
            let world = cx.world_mut();
            let late = |world: &mut World| world.register::<Late>().map_err(|e| e.code());
            assert_eq!(late(world), Err("ecs.registered_late"), "{installed}");
            let inserted = world.insert(counters[0], Late { at: 2 });
            assert_eq!(inserted.map_err(|e| e.code()), Err("ecs.registered_late"));
            let counter = Counter { count: 5, mark: 0 };
            world
                .insert(counters[0], counter)
                .expect("registered before");
            // A copy taken before, cloned into the world, has the same types without a mod's.
            let mut copy = before.clone();
            copy.clone_from(cx.world());
            assert_eq!(late(&mut copy), Err("ecs.registered_late"), "{installed}");
        }
    }

    /// A lockstep joiner taking its host's mods runs those in place of its own; taking the same
    /// mods again changes nothing.
    #[test]
    fn a_joiner_runs_its_host_s_mods_in_place_of_its_own() {
        let dir = Scratch::new("mods-joiner");
        let both = "\"read:Counter\", \"write:Counter\"";
        install(&dir.0, "counting", "Modded", both, COUNTING);
        let (mut cx, counters) = modded(&dir.0, 1);
        let mut mods = Mods::load(&mut cx, Grant::all());
        let theirs = cx.mods.applied();
        let by_ten = COUNTING
            .replace("by = 1", "by = 10")
            .replace("Counting", "Tens");
        let host = vec![ModText {
            manifest: theirs[0].manifest.replace("counting", "tens"),
            scripts: vec![("tens.luau".to_owned(), by_ten)],
        }];
        let (manifest, script) = (&host[0].manifest, &host[0].scripts[0].1);
        assert!(manifest.contains("tens.luau") && script.contains("10"));
        let run_mods = std::mem::take(&mut cx.mods);
        run_mods.take(cx.world_mut(), &host);
        run_mods.take(cx.world_mut(), &host);
        cx.mods = run_mods;
        assert_eq!(names(&cx), ["tens"]);
        mods.run(cx.world_mut());
        let counted = cx.world().get::<Counter>(counters[0]).expect("a counter");
        assert_eq!(counted.count, 10);
    }

    /// A mod's script casts rays and finds paths only if its mod may read the whole world, since a
    /// collider's position tells of every entity with one. A mod granted one type's reads fails at
    /// its cast or path with a warning, and the game goes on.
    #[test]
    fn a_mod_casts_and_finds_paths_only_reading_the_whole_world() {
        let logged = crate::testing::logged();
        let dir = Scratch::new("mods-casts");
        let reading = |name: &str, reads: &str| {
            format!(
                "local Counter = gg.component(\"Counter\")\n\
                 local {name} = gg.declare(\"{name}\", {{}}, {{ on = \"Counter\" }})\n\
                 function {name}.tick(entity, dt)\n\
                     {reads}\n\
                     entity[Counter.count] = entity[Counter.count] + 1\n\
                 end\n\
                 return {name}\n"
            )
        };
        let cast = "local hit = gg.cast_ray(vector.create(0, 5, 0), vector.create(0, -1, 0))";
        let path = "local path = gg.find_path(vector.create(0, 1, 0), vector.create(3, 1, 0))";
        let writing = "\"write:Counter\"";
        install(
            &dir.0,
            "a-reader",
            "Modded",
            &format!("\"read\", {writing}"),
            &reading("Reader", &format!("{cast}\n{path}")),
        );
        let narrow = format!("\"read:Counter\", {writing}");
        install(
            &dir.0,
            "b-narrow",
            "Modded",
            &narrow,
            &reading("Narrow", cast),
        );
        install(
            &dir.0,
            "c-pathless",
            "Modded",
            &narrow,
            &reading("Pathless", path),
        );
        let (mut cx, counters) = modded(&dir.0, 1);
        let limits = Grant::parse(["read", "write:Counter"]).expect("a grant");
        let mut mods = Mods::load(&mut cx, limits);
        assert_eq!(names(&cx), ["a-reader", "b-narrow", "c-pathless"]);
        mods.run(cx.world_mut());
        let counted = cx.world().get::<Counter>(counters[0]).expect("a counter");
        assert_eq!(counted.count, 1, "the reader's tick alone ran through");
        let codes: Vec<String> = logged.codes.try_iter().collect();
        let failed = codes.iter().filter(|code| *code == "script.error");
        assert_eq!(failed.count(), 2, "{codes:?}");
    }

    /// A mod's script tweens a field only if its mod may write `Tweens`, which hold the tween, as
    /// well as the field. One without fails at its tween with a warning, and the game goes on.
    #[test]
    fn a_mod_tweens_only_writing_tweens() {
        let logged = crate::testing::logged();
        let dir = Scratch::new("mods-tweens");
        let tweening = |name: &str| {
            format!(
                "local Counter = gg.component(\"Counter\")\n\
                 local {name} = gg.declare(\"{name}\", {{ glow = 0 }}, {{ on = \"Counter\" }})\n\
                 function {name}.tick(entity, dt)\n\
                     gg.tween(entity, {name}.glow, 1, 1)\n\
                     entity[Counter.count] = entity[Counter.count] + 1\n\
                 end\n\
                 return {name}\n"
            )
        };
        let counting = "\"read:Counter\", \"write:Counter\"";
        let granted = format!("{counting}, \"write:Tweens\"");
        install(
            &dir.0,
            "a-tweener",
            "Modded",
            &granted,
            &tweening("Tweener"),
        );
        install(&dir.0, "b-still", "Modded", counting, &tweening("Still"));
        let (mut cx, counters) = modded(&dir.0, 1);
        let limits = Grant::parse(["read:Counter", "write:Counter", "write:Tweens"]);
        let mut mods = Mods::load(&mut cx, limits.expect("a grant"));
        assert_eq!(names(&cx), ["a-tweener", "b-still"]);
        mods.run(cx.world_mut());
        let world = cx.world();
        let counted = world.get::<Counter>(counters[0]).expect("a counter");
        assert_eq!(counted.count, 1, "the tweener's tick alone ran through");
        let tweens = &world.get::<crate::Tweens>(counters[0]).expect("tweens").0;
        let tweened: Vec<_> = tweens
            .iter()
            .map(|tween| tween.component.as_str())
            .collect();
        assert_eq!(tweened, ["Tweener"]);
        let codes: Vec<String> = logged.codes.try_iter().collect();
        assert!(codes.iter().any(|code| code == "script.error"), "{codes:?}");
    }
}
