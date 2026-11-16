//! Keeps docs/LUAU.md from drifting: it has an entry for each key of the `gg` table (in a game's
//! VM and a mod's) and each diagnostic code these files name for a script, and names no `gg`
//! function that isn't there.

use std::collections::BTreeSet;

use mlua::{Table, Value as Lua_};

use super::{Held, Host, Scripts};
use crate::grant::Grant;

/// docs/LUAU.md, the reference.
const REFERENCE: &str = include_str!("../../../../docs/LUAU.md");

/// The sources whose diagnostic codes a script or mod load can see.
const SOURCES: [&str; 9] = [
    include_str!("mod.rs"),
    include_str!("api.rs"),
    include_str!("compile.rs"),
    include_str!("fields.rs"),
    include_str!("host.rs"),
    include_str!("mods.rs"),
    include_str!("entities.rs"),
    include_str!("contacts.rs"),
    include_str!("../grant.rs"),
];

/// Each key of `gg` in `host`'s VM, `gg.quat`'s as `quat.identity`.
fn keys(host: &Host) -> BTreeSet<String> {
    let gg: Table = host.lua.globals().get("gg").expect("the gg table");
    let mut keys = BTreeSet::new();
    for pair in gg.pairs::<String, Lua_>() {
        let (key, value) = pair.expect("a string key");
        if let Lua_::Table(inner) = value {
            for pair in inner.pairs::<String, Lua_>() {
                keys.insert(format!("{key}.{}", pair.expect("a string key").0));
            }
        } else {
            keys.insert(key);
        }
    }
    keys
}

/// The names `text` gives `gg` (`gg.spawn`, `gg.quat.rotate`), without the `gg.`.
fn mentioned(text: &str) -> BTreeSet<String> {
    let name = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_';
    let mut found = BTreeSet::new();
    for (at, _) in text.match_indices("gg.") {
        let before = text[..at].chars().next_back();
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            continue;
        }
        let rest = &text[at + 3..];
        let end = rest.find(|c| !name(c)).unwrap_or(rest.len());
        let head = &rest[..end];
        if head.is_empty() {
            continue;
        }
        let after = rest[end..].strip_prefix('.');
        match after {
            Some(sub) if head == "quat" && sub.starts_with(|c: char| c.is_ascii_lowercase()) => {
                let end = sub.find(|c| !name(c)).unwrap_or(sub.len());
                found.insert(format!("quat.{}", &sub[..end]));
            }
            _ => {
                found.insert(head.to_owned());
            }
        }
    }
    found
}

/// A game's VM, and a mod's.
fn hosts() -> [Host; 2] {
    let held = Held::new("reference".to_owned(), Grant::default());
    let game = Scripts::new().expect("Luau starts");
    let game = std::rc::Rc::try_unwrap(game.host).ok().expect("one owner");
    [
        game.into_inner(),
        Host::new(Some((held, 1 << 20))).expect("Luau starts"),
    ]
}

#[test]
fn docs_luau_has_an_entry_for_each_gg_function_and_none_for_one_that_is_gone() {
    let documented = mentioned(REFERENCE);
    let [game, held] = hosts();
    let keys = keys(&game);
    assert_eq!(keys, self::keys(&held), "a mod's VM has the game's gg");
    let missing: Vec<&String> = keys.difference(&documented).collect();
    assert!(
        missing.is_empty(),
        "docs/LUAU.md has no `gg.<name>` entry for: {missing:?}"
    );
    // `gg.quat` itself is a key of `gg`, a table whose own keys are listed.
    let stale: Vec<&String> = documented
        .iter()
        .filter(|name| *name != "quat" && !keys.contains(*name))
        .collect();
    assert!(
        stale.is_empty(),
        "docs/LUAU.md names `gg` functions that aren't there: {stale:?}"
    );
}

#[test]
fn docs_luau_names_each_diagnostic_code_a_script_or_a_mod_s_load_can_see() {
    let mut codes = BTreeSet::new();
    for source in SOURCES {
        // What the tests assert isn't what scripts see: `script.not_stopped` is the debugger's.
        let source = source
            .split("\n#[cfg(test)]\nmod tests")
            .next()
            .unwrap_or(source);
        for (at, _) in source.match_indices('"') {
            let rest = &source[at + 1..];
            let Some(dot) = rest.find('.') else { continue };
            let (prefix, rest) = (&rest[..dot], &rest[dot + 1..]);
            if !["script", "mod", "grant"].contains(&prefix) {
                continue;
            }
            let end = rest.find(|c: char| !(c.is_ascii_lowercase() || c == '_'));
            let end = end.unwrap_or(rest.len());
            if rest[end..].starts_with('"') && end > 0 {
                codes.insert(format!("{prefix}.{}", &rest[..end]));
            }
        }
    }
    assert!(codes.len() >= 20, "found the codes: {codes:?}");
    let missing: Vec<&String> = codes
        .iter()
        .filter(|code| !REFERENCE.contains(&format!("`{code}`")))
        .collect();
    assert!(
        missing.is_empty(),
        "docs/LUAU.md doesn't name the diagnostic codes: {missing:?}"
    );
}
