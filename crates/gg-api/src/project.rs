//! A game's project manifest, `project.ron`, beside its crate's `Cargo.toml`: how tools find the
//! game from where they run, and where its development build reads its asset root and start scene.

use std::fmt::Write as _;
use std::fs;
use std::net::Ipv6Addr;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::Failure;

/// The directories at a package's root that no build of it reads, which `gg run`'s watcher and the
/// asset [`scan`](crate::asset::scan) skip there.
pub const UNBUILT: [&str; 4] = ["target", "tests", "benches", "examples"];

/// A game's project, as its manifest gives it (ARCHITECTURE.md's Project state). Its paths are
/// relative to the manifest's directory, `/` between parts.
///
/// Its text is canonical: [`text`](Self::text) writes it a part to a line, `gg new` alone writes
/// one, and tools never rewrite it. Every string is printable ASCII without `"` or `\`, so that
/// none needs an escape.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// The game's package, which `gg run` and `gg check` build inside the project.
    pub game: String,
    /// The directory the game names its assets' files from, as `App::root` says: `.` for the
    /// manifest's own.
    pub assets: String,
    /// The folders inside `assets` holding the game's scenes.
    pub scenes: Vec<String>,
    /// The scene the game starts in, a `.ron` file in a scenes folder.
    pub start: String,
    /// The relay the game's own sessions meet through, `host:port`, as `relay.example.com:7800`,
    /// unless a build is given `--relay`; its package holds it for builds without `dev`. `None`,
    /// left out of the manifest, for none.
    #[serde(default)]
    pub relay: Option<String>,
    /// The game's icon, a 256 by 256 PNG, by its path from the manifest's directory, as `icon.png`:
    /// the Windows executable's, the window's (gg-api's `build`) and a macOS bundle's. `None`, left
    /// out of the manifest, for none. Its package doesn't hold it.
    #[serde(default)]
    pub icon: Option<String>,
}

impl Project {
    /// The manifest's file name.
    pub const FILE: &str = "project.ron";

    /// Reads a manifest's text, and checks it as [`check`](Self::check) does.
    ///
    /// Fails with `project.bad`, starting with the line and column where ron stopped, for text that
    /// isn't a project's, as one with an unknown field or missing one; and as `check` does.
    pub fn parse(text: &str) -> Result<Self, Failure> {
        let project: Self = ron::from_str(text).map_err(|error| {
            let at = error.span.end;
            let message = format!("{}:{}: {}", at.line, at.col, error.code);
            Failure::new("project.bad", message)
        })?;
        project.check()?;
        Ok(project)
    }

    /// Reads the manifest in directory `dir`, as [`parse`](Self::parse) does.
    ///
    /// Fails with `project.read` if the file can't be read, naming its path, and as `parse` does,
    /// the path starting its message.
    pub fn read(dir: &Path) -> Result<Self, Failure> {
        let path = dir.join(Self::FILE);
        let text = fs::read_to_string(&path).map_err(|error| {
            let message = format!("can't read the project {}: {error}", path.display());
            Failure::new("project.read", message)
        })?;
        Self::parse(&text).map_err(|failure| {
            let message = format!("{}: {}", path.display(), failure.message);
            Failure::new(failure.code, message)
        })
    }

    /// Checks the project's fields:
    ///
    /// - `game`, a package's name as Cargo takes it, ASCII letters, digits, `-` and `_`, starting
    ///   with a letter or `_`; and one a folder may have on every OS, not a Windows device's, as
    ///   `gg new` makes the game's folder of that name in the workspace, every member of which
    ///   Cargo reads;
    /// - each path relative, its parts named, neither `.` nor `..`, `/` between them; but
    ///   `assets` may be `.`;
    /// - `scenes`, at least one folder, each once and inside `assets`; `start`, a `.ron` file in
    ///   one of them;
    /// - `relay`, if any, `host:port`: its host letters, digits, `.` and `-`, or an IPv6 address
    ///   in brackets, and its port 1 to 65535;
    /// - `icon`, if any, a relative path ending in `.png`;
    /// - every string printable ASCII without `"` or `\`.
    ///
    /// Fails with `project.bad`, naming the field.
    pub fn check(&self) -> Result<(), Failure> {
        let bad = |message: String| Err(Failure::new("project.bad", message));
        let strings = [("game", &self.game), ("assets", &self.assets)].into_iter();
        let strings = strings.chain(self.scenes.iter().map(|scene| ("scenes", scene)));
        let strings = strings.chain([("start", &self.start)]);
        let strings = strings.chain(self.relay.iter().map(|relay| ("relay", relay)));
        for (field, value) in strings.chain(self.icon.iter().map(|icon| ("icon", icon))) {
            if let Some(c) =
                (value.chars()).find(|&c| !(' '..='~').contains(&c) || "\"\\".contains(c))
            {
                return bad(format!(
                    "`{field}` holds {c:?}, but its strings take printable ASCII alone, \
                     without `\"` or `\\`"
                ));
            }
        }
        let named = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        let first = |c: char| c.is_ascii_alphabetic() || c == '_';
        if !(self.game.starts_with(first) && self.game.chars().all(named)) || device(&self.game) {
            return bad(format!(
                "`game` is `{}`, which isn't a package's name: ASCII letters, digits, `-` and `_`, \
                 starting with a letter or `_`, and not a name Windows keeps for a device, as \
                 `con` or `nul`",
                self.game
            ));
        }
        if self.assets != "." {
            relative("assets", &self.assets)?;
        }
        if self.scenes.is_empty() {
            return bad("`scenes` names no folder, where the game's scenes are".to_owned());
        }
        for (index, scene) in self.scenes.iter().enumerate() {
            relative("scenes", scene)?;
            if !inside(scene, &self.assets) {
                let assets = &self.assets;
                return bad(format!(
                    "`scenes` holds `{scene}`, which isn't inside `{assets}`"
                ));
            }
            if self.scenes[..index].contains(scene) {
                return bad(format!("`scenes` holds `{scene}` twice"));
            }
        }
        relative("start", &self.start)?;
        if !self.start.ends_with(".ron") {
            let start = &self.start;
            return bad(format!(
                "`start` is `{start}`, which isn't a scene's `.ron` file"
            ));
        }
        if !(self.scenes.iter()).any(|scene| inside(&self.start, scene)) {
            let start = &self.start;
            return bad(format!(
                "`start` is `{start}`, which isn't in a scenes folder"
            ));
        }
        if let Some(relay) = self.relay.as_deref().filter(|&relay| !is_relay(relay)) {
            return bad(format!(
                "`relay` is `{relay}`, which isn't a relay's `host:port`: its host letters, digits, \
                 `.` and `-`, or an IPv6 address in brackets, and its port 1 to 65535"
            ));
        }
        if let Some(icon) = &self.icon {
            relative("icon", icon)?;
            if !icon.ends_with(".png") {
                return bad(format!("`icon` is `{icon}`, which isn't a `.png` file"));
            }
        }
        Ok(())
    }

    /// The game's name as a player sees it: its package's, capitalized, as `Shards`, which names a
    /// shipped folder's executable, package and macOS bundle.
    pub fn name(&self) -> String {
        let mut chars = self.game.chars();
        let first = chars.next().map(|first| first.to_ascii_uppercase());
        first.into_iter().chain(chars).collect()
    }

    /// The manifest's canonical text, which reads back as it is for a project
    /// [`check`](Self::check) passes: a part to a line, four spaces to a level, a comma after each
    /// part, lines ending in `\n`, and no line for a relay or an icon it hasn't.
    pub fn text(&self) -> String {
        let (game, assets, start) = (&self.game, &self.assets, &self.start);
        let mut text =
            format!("(\n    game: \"{game}\",\n    assets: \"{assets}\",\n    scenes: [\n");
        for scene in &self.scenes {
            let _ = writeln!(text, "        \"{scene}\",");
        }
        let _ = writeln!(text, "    ],\n    start: \"{start}\",");
        if let Some(relay) = &self.relay {
            let _ = writeln!(text, "    relay: Some(\"{relay}\"),");
        }
        if let Some(icon) = &self.icon {
            let _ = writeln!(text, "    icon: Some(\"{icon}\"),");
        }
        text.push_str(")\n");
        text
    }

    /// The directory of the project `from` is in: `from` or its nearest ancestor holding a
    /// manifest and a `Cargo.toml`, the game crate's, so that a stray manifest isn't taken for one;
    /// `None` if none does.
    pub fn find(from: &Path) -> Option<PathBuf> {
        let project = (from.ancestors())
            .find(|dir| dir.join(Self::FILE).is_file() && dir.join("Cargo.toml").is_file());
        project.map(Path::to_owned)
    }

    /// The asset root of the project whose manifest is in `dir`: `dir` itself for `.`.
    pub fn assets_dir(&self, dir: &Path) -> PathBuf {
        under(dir, &self.assets)
    }

    /// The start scene's path, for the manifest in `dir`.
    pub fn start_path(&self, dir: &Path) -> PathBuf {
        under(dir, &self.start)
    }

    /// The scenes folders' paths, for the manifest in `dir`.
    pub fn scene_dirs(&self, dir: &Path) -> Vec<PathBuf> {
        self.scenes.iter().map(|scene| under(dir, scene)).collect()
    }

    /// The icon's path, if any, for the manifest in `dir`.
    pub fn icon_path(&self, dir: &Path) -> Option<PathBuf> {
        self.icon.as_deref().map(|icon| under(dir, icon))
    }
}

/// Checks that field `field`'s `path` [`is_relative`], failing with `project.bad`, naming the
/// field.
fn relative(field: &str, path: &str) -> Result<(), Failure> {
    if is_relative(path) {
        return Ok(());
    }
    let message = format!(
        "`{field}` holds `{path}`, which isn't a path inside the project's directory: its parts \
         named, neither `.` nor `..`, with `/` between them"
    );
    Err(Failure::new("project.bad", message))
}

/// Whether `path` is inside a directory, relative to it: its parts named, neither `.` nor `..`, `/`
/// between them, each one [`Component::Normal`] as the OS reads it. It has no `\`, `:` or control
/// character on any OS, as Windows reads `\` as a separator and `:` as a drive's, so that a path
/// written on one OS, as a fetch list on Linux, stays inside on every other.
pub(crate) fn is_relative(path: &str) -> bool {
    let named = |part: &str| {
        let mut components = Path::new(part).components();
        matches!(
            (components.next(), components.next()),
            (Some(Component::Normal(_)), None)
        )
    };
    let refused = |c: char| c.is_control() || c == '\\' || c == ':';
    !path.contains(refused) && path.split('/').all(named)
}

/// Whether `address` is a relay's `host:port`: the host letters, digits, `.` and `-`, a name or an
/// IPv4 address, or an IPv6 address in brackets; the port 1 to 65535 in digits alone.
fn is_relay(address: &str) -> bool {
    let Some((host, port)) = address.rsplit_once(':') else {
        return false;
    };
    let port = !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port > 0);
    let host = match host.strip_prefix('[').and_then(|ip| ip.strip_suffix(']')) {
        Some(ip) => ip.parse::<Ipv6Addr>().is_ok(),
        None => {
            !host.is_empty()
                && (host.chars()).all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        }
    };
    port && host
}

/// Whether `name` is a Windows device's, in any case, as `CON` or `com1`, which no file or folder
/// there may have.
fn device(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let numbered = |prefix: &str| {
        (name.strip_prefix(prefix))
            .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit())
    };
    matches!(name.as_str(), "con" | "prn" | "aux" | "nul") || numbered("com") || numbered("lpt")
}

/// Whether `path` is inside directory `dir`, both relative paths: every path is inside `.`.
fn inside(path: &str, dir: &str) -> bool {
    dir == "." || (path.strip_prefix(dir)).is_some_and(|rest| rest.starts_with('/'))
}

/// Relative `path` joined to `dir` a part at a time, so its separators are the OS's; `dir` itself
/// for `.`. A path [`is_relative`] passes, as a manifest's and a fetch list's checks keep theirs,
/// joins only named parts, which no join takes outside `dir`.
pub(crate) fn under(dir: &Path, path: &str) -> PathBuf {
    let parts = path.split('/').filter(|&part| part != ".");
    parts.fold(dir.to_owned(), |under, part| under.join(part))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHARDS: &str = "(\n    game: \"shards\",\n    assets: \".\",\n    scenes: [\n        \
                          \"scenes\",\n    ],\n    start: \"scenes/arena.ron\",\n)\n";

    fn shards() -> Project {
        Project {
            game: "shards".to_owned(),
            assets: ".".to_owned(),
            scenes: vec!["scenes".to_owned()],
            start: "scenes/arena.ron".to_owned(),
            relay: None,
            icon: None,
        }
    }

    #[test]
    fn a_project_reads_and_writes_back_to_its_bytes() {
        assert_eq!(shards().text(), SHARDS);
        assert_eq!(Project::parse(SHARDS), Ok(shards()));
        let nested = Project {
            game: "Big_Game-2".to_owned(),
            assets: "content".to_owned(),
            scenes: vec!["content/levels".to_owned(), "content/menus".to_owned()],
            start: "content/menus/title/main.ron".to_owned(),
            relay: None,
            icon: None,
        };
        assert_eq!(Project::parse(&nested.text()), Ok(nested));
        // A relay, its line after the start scene's.
        let relayed = SHARDS.replace(")\n", "    relay: Some(\"relay.example.com:7800\"),\n)\n");
        let project = Project {
            relay: Some("relay.example.com:7800".to_owned()),
            ..shards()
        };
        assert_eq!(project.text(), relayed);
        assert_eq!(Project::parse(&relayed), Ok(project));
        for relay in [
            "203.0.113.7:7800",
            "[2001:db8::7]:7800",
            "a-b.c:1",
            "x:65535",
        ] {
            let project = Project {
                relay: Some(relay.to_owned()),
                ..shards()
            };
            assert_eq!(Project::parse(&project.text()), Ok(project), "{relay}");
        }
        // An icon, its line after the relay's.
        let iconic = relayed.replace(")\n", "    icon: Some(\"art/icon.png\"),\n)\n");
        let project = Project {
            relay: Some("relay.example.com:7800".to_owned()),
            icon: Some("art/icon.png".to_owned()),
            ..shards()
        };
        assert_eq!(project.text(), iconic);
        assert_eq!(Project::parse(&iconic), Ok(project));
        let alone = SHARDS.replace(")\n", "    icon: Some(\"icon.png\"),\n)\n");
        assert_eq!(
            Project::parse(&alone).map(|project| project.icon),
            Ok(Some("icon.png".to_owned()))
        );
    }

    #[test]
    fn a_games_name_is_its_packages_capitalized() {
        assert_eq!(shards().name(), "Shards");
        let named = |game: &str| {
            Project {
                game: game.to_owned(),
                ..shards()
            }
            .name()
        };
        assert_eq!(named("foxball"), "Foxball");
        assert_eq!(named("my_game-2"), "My_game-2");
        assert_eq!(named("_hidden"), "_hidden");
    }

    #[test]
    fn a_project_refuses_what_its_fields_cant_be() {
        let refused = |text: &str| Project::parse(text).expect_err(text);
        let unknown = SHARDS.replace("    start:", "    name: \"Shards\",\n    start:");
        let missing = SHARDS.replace("    start: \"scenes/arena.ron\",\n", "");
        for (text, field) in [(unknown, "name"), (missing, "start")] {
            let failure = refused(&text);
            assert_eq!(failure.code, "project.bad");
            assert!(failure.message.contains(field), "{failure:?}");
        }
        type Change = fn(&mut Project);
        let changed = |change: Change| {
            let mut project = shards();
            change(&mut project);
            project
        };
        let cases: [(Change, &str); 30] = [
            (|p| p.icon = Some("/icon.png".to_owned()), "icon"),
            (|p| p.icon = Some("../icon.png".to_owned()), "icon"),
            (|p| p.icon = Some("art/../icon.png".to_owned()), "icon"),
            (|p| p.icon = Some("icon.ico".to_owned()), "icon"),
            (|p| p.icon = Some("icon".to_owned()), "icon"),
            (|p| p.relay = Some("relay.example.com".to_owned()), "relay"),
            (
                |p| p.relay = Some("relay.example.com:0".to_owned()),
                "relay",
            ),
            (
                |p| p.relay = Some("relay.example.com:70000".to_owned()),
                "relay",
            ),
            (
                |p| p.relay = Some("relay.example.com:+80".to_owned()),
                "relay",
            ),
            (|p| p.relay = Some("relay.example.com:".to_owned()), "relay"),
            (|p| p.relay = Some(":7800".to_owned()), "relay"),
            (|p| p.relay = Some("a b:7800".to_owned()), "relay"),
            (|p| p.relay = Some("a/b:7800".to_owned()), "relay"),
            (|p| p.relay = Some("2001:db8::7:7800".to_owned()), "relay"),
            (|p| p.relay = Some("[not an ip]:7800".to_owned()), "relay"),
            (|p| p.start = "/scenes/arena.ron".to_owned(), "start"),
            (|p| p.start = "C:/scenes/arena.ron".to_owned(), "start"),
            (|p| p.start = "scenes/../arena.ron".to_owned(), "start"),
            (|p| p.start = "scenes//arena.ron".to_owned(), "start"),
            (|p| p.start = "levels/arena.ron".to_owned(), "start"),
            (|p| p.start = "scenes/arena.txt".to_owned(), "start"),
            (|p| p.assets = "content".to_owned(), "scenes"),
            (|p| p.scenes.clear(), "scenes"),
            (|p| p.scenes.push("scenes".to_owned()), "scenes"),
            (|p| p.game = "2shards".to_owned(), "game"),
            (|p| p.game = "sh ards".to_owned(), "game"),
            // Which Cargo refuses, and every command in the workspace with it, as the folder `gg
            // new` makes of it is a member; and names a folder can't have on Windows.
            (|p| p.game = "-shards".to_owned(), "game"),
            (|p| p.game = "nul".to_owned(), "game"),
            (|p| p.game = "Con".to_owned(), "game"),
            (|p| p.game = "COM1".to_owned(), "game"),
        ];
        for (change, field) in cases {
            let project = changed(change);
            for failure in [project.check().expect_err(field), refused(&project.text())] {
                assert_eq!(failure.code, "project.bad", "{project:?}");
                assert!(failure.message.contains(field), "{project:?}: {failure:?}");
            }
        }
        // Names that start as a device's do, or that start with `_`, are a game's.
        for game in ["console", "com10", "lpt", "_nul"] {
            let project = Project {
                game: game.to_owned(),
                ..shards()
            };
            assert_eq!(project.check(), Ok(()), "{game}");
        }
        // Strings that would need escapes, which reach a project only through ron's.
        for (from, to, field) in [
            ("\"shards\"", r#""sh\"ards""#, "game"),
            ("\"scenes\",", r#""scenes\\levels","#, "scenes"),
        ] {
            let failure = refused(&SHARDS.replace(from, to));
            assert_eq!(failure.code, "project.bad");
            assert!(failure.message.contains(field), "{failure:?}");
        }
        let syntax = refused("(\n    game: shards,\n)\n");
        assert!(syntax.message.starts_with("2:"), "{syntax:?}");
    }

    /// Each part must be one named part on every OS, wherever its manifest or fetch list was
    /// written: no `\`, a drive, NUL or another control character. `under` keeps it inside.
    #[test]
    fn a_relative_path_stays_inside_its_directory_on_every_os() {
        for path in [
            "models",
            "models/sponza/textures/a b.png",
            "games/x-y_z/scene.ron",
        ] {
            assert!(is_relative(path), "{path:?}");
            let dir = Path::new("project");
            assert!(under(dir, path).starts_with(dir), "{path:?}");
        }
        for path in [
            r"..\x",
            r"\x",
            r"a\..\..\b",
            r"models\x.gltf",
            "C:x",
            "C:/x",
            "/x",
            "a//b",
            "a/./b",
            "a/../b",
            "",
            "a\0b",
            "a\u{1b}b",
            "a/b\n",
        ] {
            assert!(!is_relative(path), "{path:?}");
        }
    }

    #[test]
    fn a_project_is_found_beside_a_cargo_manifest_above_where_tools_run() {
        let root = std::env::temp_dir().join(format!("gg-api-project-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (game, stray) = (root.join("games/game"), root.join("stray"));
        fs::create_dir_all(game.join("src")).unwrap();
        fs::create_dir_all(&stray).unwrap();
        fs::write(game.join(Project::FILE), SHARDS).unwrap();
        fs::write(game.join("Cargo.toml"), "").unwrap();
        fs::write(stray.join(Project::FILE), SHARDS).unwrap();
        assert_eq!(Project::find(&game.join("src")), Some(game.clone()));
        assert_eq!(Project::find(&stray), None);
        let project = Project::read(&game).expect("the project");
        assert_eq!(project.assets_dir(&game), game);
        assert_eq!(
            project.start_path(&game),
            game.join("scenes").join("arena.ron")
        );
        assert_eq!(project.scene_dirs(&game), [game.join("scenes")]);
        let missing = Project::read(&stray.join("none"))
            .err()
            .map(|failure| failure.code);
        assert_eq!(missing.as_deref(), Some("project.read"));
        fs::remove_dir_all(&root).unwrap();
    }
}
