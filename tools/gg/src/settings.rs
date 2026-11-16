//! The settings a game keeps as its engine has them, which `gg check` compares: the methods clippy
//! refuses (clippy.toml, among them the float methods whose bits differ by platform) and the Rust
//! release (rust-toolchain.toml, which `gg new` copies). The engine's are read from the checkout
//! the game builds against, not gg's embedded copies, so a gg of another revision compares the
//! right files. In the engine's workspace, a game's files are the engine's.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::Document;

/// A game's setting that isn't its engine's: what's amiss, and its file, or the directory searched
/// if none was found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Drift {
    pub(crate) message: String,
    pub(crate) path: PathBuf,
}

/// How a game's settings differ from the engine's at `engine`: the clippy.toml clippy finds from
/// `clippy_from` up (`CLIPPY_CONF_DIR`, else the crate's directory) lacks a banned method, or the
/// rust-toolchain.toml rustup finds from `toolchain_from` up (where cargo builds) pins another
/// release. An unreadable or unparsable file, the engine's too, is a drift. Rustup's overrides
/// (`RUSTUP_TOOLCHAIN`, `rustup override`) and older `rust-toolchain` file (`gg new` doesn't write
/// it) aren't read.
pub(crate) fn drifts(clippy_from: &Path, toolchain_from: &Path, engine: &Path) -> Vec<Drift> {
    [
        clippy(clippy_from, engine),
        toolchain(toolchain_from, engine),
    ]
    .into_iter()
    .filter_map(Result::err)
    .collect()
}

fn clippy(from: &Path, engine: &Path) -> Result<(), Drift> {
    let engines = engine.join("clippy.toml");
    let bans = banned(&parsed(&engines)?);
    // Clippy's own order, in each directory.
    let Some(games) = found(from, &[".clippy.toml", "clippy.toml"]) else {
        let message = format!(
            "no clippy.toml applies to the game, in {} or above, so clippy allows the methods {} \
             bans: copy it there",
            from.display(),
            engines.display()
        );
        return Err(drift(message, from));
    };
    let theirs = banned(&parsed(&games)?);
    let lacking: Vec<&str> = bans.difference(&theirs).map(String::as_str).collect();
    if lacking.is_empty() {
        return Ok(());
    }
    let message = format!(
        "{} lacks {} of the methods {} bans, which clippy then allows: {}; copy the engine's",
        games.display(),
        lacking.len(),
        engines.display(),
        lacking.join(", ")
    );
    Err(drift(message, &games))
}

fn toolchain(from: &Path, engine: &Path) -> Result<(), Drift> {
    let engines = engine.join("rust-toolchain.toml");
    let engine_doc = parsed(&engines)?;
    let pin = channel(&engine_doc);
    let Some(games) = found(from, &["rust-toolchain.toml"]) else {
        let message = format!(
            "no rust-toolchain.toml pins the game's Rust release, in {} or above, where {} pins \
             {}: copy it there",
            from.display(),
            engines.display(),
            pin.unwrap_or("none")
        );
        return Err(drift(message, from));
    };
    let game_doc = parsed(&games)?;
    let theirs = channel(&game_doc);
    if theirs == pin {
        return Ok(());
    }
    let message = format!(
        "{} pins Rust {}, where {} pins {}: copy the engine's",
        games.display(),
        theirs.unwrap_or("no release"),
        engines.display(),
        pin.unwrap_or("none")
    );
    Err(drift(message, &games))
}

/// The paths in `clippy`'s `disallowed-methods`: strings or tables with a `path`, inline or not.
fn banned(clippy: &Document<String>) -> BTreeSet<String> {
    let Some(methods) = clippy.get("disallowed-methods") else {
        return BTreeSet::new();
    };
    let inline = (methods.as_array().into_iter().flatten()).filter_map(|method| {
        method
            .as_str()
            .or_else(|| method.as_inline_table()?.get("path")?.as_str())
    });
    let tables = (methods.as_array_of_tables().into_iter().flatten())
        .filter_map(|method| method.get("path")?.as_str());
    inline.chain(tables).map(str::to_owned).collect()
}

fn channel(toolchain: &Document<String>) -> Option<&str> {
    toolchain.get("toolchain")?.get("channel")?.as_str()
}

fn parsed(path: &Path) -> Result<Document<String>, Drift> {
    let text = fs::read_to_string(path).map_err(|error| {
        let message = format!("can't read {}: {error}", path.display());
        drift(message, path)
    })?;
    Document::parse(text).map_err(|error| {
        let message = format!("can't read {} as TOML: {error}", path.display());
        drift(message, path)
    })
}

/// The first of `names` in `dir`, else in the nearest ancestor holding one.
fn found(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    (dir.ancestors())
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

fn drift(message: String, path: &Path) -> Drift {
    Drift {
        message,
        path: path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An engine checkout banning `f32::sin` and `f64::cos`, pinning 1.99.1.
    fn engine(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gg-settings-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let engine = dir.join("engine");
        fs::create_dir_all(&engine).expect("a directory");
        let clippy = "disallowed-methods = [\n    { path = \"f32::sin\", reason = \"libm's\" },\n    \
                      { path = \"f64::cos\", reason = \"libm's\" },\n]\n";
        fs::write(engine.join("clippy.toml"), clippy).expect("written");
        let toolchain = "[toolchain]\nchannel = \"1.99.1\"\ncomponents = [\"clippy\"]\n";
        fs::write(engine.join("rust-toolchain.toml"), toolchain).expect("written");
        engine
    }

    fn game(engine: &Path, files: &[(&str, &str)]) -> PathBuf {
        let game = engine.with_file_name("game");
        let _ = fs::remove_dir_all(&game);
        fs::create_dir_all(&game).expect("a directory");
        for (name, text) in files {
            fs::write(game.join(name), text).expect("written");
        }
        game
    }

    /// A game in the engine's workspace, and one with its bans (as tables, plus one) and release.
    #[test]
    fn a_game_with_the_engines_settings_drifts_in_nothing() {
        let engine = engine("same");
        let inside = engine.join("games").join("shards");
        fs::create_dir_all(&inside).expect("a directory");
        let toolchain = "[toolchain]\nchannel = \"1.99.1\"\n";
        let clippy = "[[disallowed-methods]]\npath = \"f32::tan\"\n\n\
                      [[disallowed-methods]]\npath = \"f64::cos\"\n\n\
                      [[disallowed-methods]]\npath = \"f32::sin\"\n";
        let outside = game(
            &engine,
            &[("clippy.toml", clippy), ("rust-toolchain.toml", toolchain)],
        );
        let found = [
            drifts(&inside, &inside, &engine),
            drifts(&outside, &outside, &engine),
        ];
        let _ = fs::remove_dir_all(engine.parent().expect("the test's"));
        assert_eq!(found, [vec![], vec![]]);
    }

    /// A game lacking a ban or pinning another release drifts, naming the file. `.clippy.toml`
    /// is read before `clippy.toml`, as clippy does; both are found from below the game's.
    #[test]
    fn a_game_lacking_a_ban_or_pinning_another_release_drifts() {
        let engine = engine("apart");
        let shadowed = "disallowed-methods = [{ path = \"f32::sin\" }, { path = \"f64::cos\" }]\n";
        let toolchain = "[toolchain]\nchannel = \"1.98.0\"\n";
        let game = game(
            &engine,
            &[
                (".clippy.toml", "disallowed-methods = [\"f32::sin\"]\n"),
                ("clippy.toml", shadowed),
                ("rust-toolchain.toml", toolchain),
            ],
        );
        let found = drifts(&game, &game, &engine);
        let built_in = game.join("src");
        fs::create_dir_all(&built_in).expect("a directory");
        let from_below = drifts(&built_in, &built_in, &engine);
        let _ = fs::remove_dir_all(engine.parent().expect("the test's"));
        let paths: Vec<&Path> = found.iter().map(|drift| drift.path.as_path()).collect();
        let (clippy, toolchain) = (game.join(".clippy.toml"), game.join("rust-toolchain.toml"));
        assert_eq!(paths, [clippy.as_path(), toolchain.as_path()]);
        assert!(
            found[0].message.contains("lacks 1 of the methods"),
            "{}",
            found[0].message
        );
        assert!(
            found[0].message.ends_with(": f64::cos; copy the engine's"),
            "{}",
            found[0].message
        );
        assert!(
            found[1].message.contains("pins Rust 1.98.0, where"),
            "{}",
            found[1].message
        );
        assert!(
            found[1].message.contains("pins 1.99.1: copy"),
            "{}",
            found[1].message
        );
        assert_eq!(from_below, found);
    }

    /// A game without the files or with non-TOML ones, and an engine without its own, drift.
    #[test]
    fn missing_or_unreadable_settings_drift() {
        let engine = engine("missing");
        let bare = game(&engine, &[]);
        let missing = drifts(&bare, &bare, &engine);
        let broken = game(
            &engine,
            &[
                ("clippy.toml", "disallowed-methods = ["),
                ("rust-toolchain.toml", ""),
            ],
        );
        let unreadable = drifts(&broken, &broken, &engine);
        fs::remove_file(engine.join("clippy.toml")).expect("removed");
        let engineless = drifts(&broken, &broken, &engine);
        let _ = fs::remove_dir_all(engine.parent().expect("the test's"));
        let messages = |drifts: &[Drift]| -> Vec<String> {
            drifts.iter().map(|drift| drift.message.clone()).collect()
        };
        let missing = messages(&missing);
        assert!(
            missing[0].starts_with("no clippy.toml applies to the game"),
            "{missing:?}"
        );
        assert!(
            missing[1].starts_with("no rust-toolchain.toml pins"),
            "{missing:?}"
        );
        assert_eq!(missing.len(), 2);
        let unreadable = messages(&unreadable);
        assert!(
            unreadable[0].contains("clippy.toml as TOML"),
            "{unreadable:?}"
        );
        assert!(
            unreadable[1].contains("pins Rust no release, where"),
            "{unreadable:?}"
        );
        let engineless = messages(&engineless);
        let engines = engine.join("clippy.toml").display().to_string();
        assert!(
            engineless[0].starts_with(&format!("can't read {engines}: ")),
            "{engineless:?}"
        );
    }
}
