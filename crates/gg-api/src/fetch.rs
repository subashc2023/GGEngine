//! A project's fetch list, `fetch.ron` beside its manifest: the files its game uses that its
//! repository doesn't hold, as models whose licenses keep them out. `gg fetch` fetches each from a
//! folder at a URL pinned to what it holds, as a repository's at a commit, checks its SHA-256, and
//! puts it in a project folder the repository ignores.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Failure;
use crate::asset::{AssetId, AssetKind};
use crate::project::{is_relative, under};

/// A project's fetch list: [`Source`]s, each fetched from one folder into one, its files in path
/// order.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Fetch(pub Vec<Source>);

/// Files fetched from one folder at a URL into one of the project's.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// The folder's `https://` URL, without a `/` at its end, pinned to what it holds, as a
    /// repository's files are at a commit, so that its files never change.
    pub from: String,
    /// The project folder they go into, from the list's directory, `/` between parts, which the
    /// repository should ignore.
    pub into: String,
    /// Each file's path from `from` and `into` alike, `/` between parts, and its SHA-256 in
    /// lowercase hex.
    pub files: BTreeMap<String, String>,
    /// The asset ID of each asset of `files` a scene names by ID, by its path in `files`. `gg
    /// fetch` writes it in the asset's ID file, so that every checkout gives it the ID the scene
    /// names. None by default.
    #[serde(default)]
    pub ids: BTreeMap<String, String>,
}

/// A file a fetch list names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File<'a> {
    pub url: String,
    /// Its path from the list's directory, `/` between parts.
    pub path: String,
    /// In lowercase hex.
    pub sha256: &'a str,
    /// The asset ID the list gives it, if any.
    pub id: Option<AssetId>,
}

impl File<'_> {
    /// The file's path, for the list in directory `dir`.
    pub fn at(&self, dir: &Path) -> PathBuf {
        under(dir, &self.path)
    }
}

impl Fetch {
    /// The list's file name.
    pub const FILE: &str = "fetch.ron";

    /// Reads a list's text, and checks it as [`check`](Self::check) does.
    ///
    /// Fails with `fetch.bad`, starting with the line and column where ron stopped, for text that
    /// isn't a fetch list's, as one with an unknown field or missing one; and as `check` does.
    pub fn parse(text: &str) -> Result<Self, Failure> {
        let fetch: Self = ron::from_str(text).map_err(|error| {
            let at = error.span.end;
            let message = format!("{}:{}: {}", at.line, at.col, error.code);
            Failure::new("fetch.bad", message)
        })?;
        fetch.check()?;
        Ok(fetch)
    }

    /// Reads the list in directory `dir`, as [`parse`](Self::parse) does.
    ///
    /// Fails with `fetch.read` if the file can't be read, naming its path, and as `parse` does,
    /// the path starting its message.
    pub fn read(dir: &Path) -> Result<Self, Failure> {
        let path = dir.join(Self::FILE);
        let text = fs::read_to_string(&path).map_err(|error| {
            let message = format!("can't read the fetch list {}: {error}", path.display());
            Failure::new("fetch.read", message)
        })?;
        Self::parse(&text).map_err(|failure| {
            let message = format!("{}: {}", path.display(), failure.message);
            Failure::new(failure.code, message)
        })
    }

    /// Checks the list:
    ///
    /// - each source's `from`, an `https://` URL of printable ASCII, without spaces, `"`, `\` or a
    ///   `/` at its end;
    /// - its `into` and its files' paths relative, their parts named, neither `.` nor `..`, `/`
    ///   between them, without `\`, `:` or a control character, so that they stay inside the
    ///   project's directory on every OS;
    /// - each file's SHA-256, 64 lowercase hex digits; no path into the project twice;
    /// - each of `ids`, an [`AssetId`] of an asset of `files`, no two the same.
    ///
    /// Fails with `fetch.bad`, naming what's wrong.
    pub fn check(&self) -> Result<(), Failure> {
        let bad = |message: String| Err(Failure::new("fetch.bad", message));
        let mut paths = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for source in &self.0 {
            let from = &source.from;
            let printable = (from.chars()).all(|c| ('!'..='~').contains(&c) && !"\"\\".contains(c));
            if !(from.starts_with("https://") && printable && !from.ends_with('/')) {
                return bad(format!(
                    "`from` is `{from}`, which isn't a folder's URL: `https://`, printable ASCII \
                     without spaces, `\"` or `\\`, and without a `/` at its end"
                ));
            }
            let into = &source.into;
            if !is_relative(into) {
                return bad(format!(
                    "`into` is `{into}`, which isn't a path inside the project's directory: its \
                     parts named, neither `.` nor `..`, with `/` between them, without `\\`, `:` \
                     or a control character"
                ));
            }
            for (name, sha256) in &source.files {
                if !is_relative(name) {
                    return bad(format!(
                        "`files` names `{name}`, which isn't a path inside `{into}`: its parts \
                         named, neither `.` nor `..`, with `/` between them, without `\\`, `:` \
                         or a control character"
                    ));
                }
                let hex = |b: u8| matches!(b, b'0'..=b'9' | b'a'..=b'f');
                if !(sha256.len() == 64 && sha256.bytes().all(hex)) {
                    return bad(format!(
                        "`{name}`'s SHA-256 is `{sha256}`, not 64 lowercase hex digits"
                    ));
                }
                let path = format!("{into}/{name}");
                if !paths.insert(path.clone()) {
                    return bad(format!("`{path}` is fetched twice"));
                }
            }
            for (name, id) in &source.ids {
                let parsed = id.parse::<AssetId>().map_err(|failure| {
                    let message = format!("`ids` gives `{name}` {}", failure.message);
                    Failure::new("fetch.bad", message)
                })?;
                if !(source.files.contains_key(name) && AssetKind::of(Path::new(name)).is_some()) {
                    return bad(format!(
                        "`ids` gives `{name}` an ID, but it isn't a texture, a model, a font or a \
                         prefab that `files` names"
                    ));
                }
                if !ids.insert(parsed) {
                    return bad(format!("`ids` gives {id} twice"));
                }
            }
        }
        Ok(())
    }

    /// The files the list names, source by source, each's in path order.
    pub fn files(&self) -> impl Iterator<Item = File<'_>> {
        self.0.iter().flat_map(|source| {
            (source.files.iter()).map(|(name, sha256)| File {
                url: format!("{}/{name}", source.from),
                path: format!("{}/{name}", source.into),
                sha256,
                id: (source.ids.get(name)).and_then(|id| id.parse().ok()),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn list(from: &str, into: &str, name: &str, hash: &str) -> String {
        format!(
            "[\n    (\n        from: \"{from}\",\n        into: \"{into}\",\n        files: {{\n            \
             \"{name}\": \"{hash}\",\n        }},\n    ),\n]\n"
        )
    }

    /// `list`'s source given `ids`, the text of its `ids` map's entries.
    fn with_ids(list: &str, ids: &str) -> String {
        list.replace(
            "        },\n    ),",
            &format!("        }},\n        ids: {{\n{ids}        }},\n    ),"),
        )
    }

    #[test]
    fn a_fetch_list_names_each_files_url_path_and_id() {
        let text = list("https://example.com/a/b", "models/x", "y/z.gltf", HASH);
        let fetch = Fetch::parse(&text).expect("a list");
        let files: Vec<File<'_>> = fetch.files().collect();
        assert_eq!(
            files,
            [File {
                url: "https://example.com/a/b/y/z.gltf".to_owned(),
                path: "models/x/y/z.gltf".to_owned(),
                sha256: HASH,
                id: None,
            }]
        );
        let dir = Path::new("project");
        assert_eq!(
            files[0].at(dir),
            dir.join("models").join("x").join("y").join("z.gltf")
        );
        let id = "5f3c9a1e07b2d846";
        let given = with_ids(&text, &format!("            \"y/z.gltf\": \"{id}\",\n"));
        let fetch = Fetch::parse(&given).expect("a list with an ID");
        let ids: Vec<_> = fetch.files().map(|file| file.id).collect();
        assert_eq!(ids, [Some(id.parse().expect("an ID"))]);
        assert_eq!(Fetch::parse("[]\n"), Ok(Fetch(Vec::new())));
    }

    #[test]
    fn a_fetch_list_refuses_what_its_fields_cant_be() {
        let good = ("https://example.com/a", "models/x", "z.bin", HASH);
        let cases = [
            ("http://example.com/a", good.1, good.2, good.3, "from"),
            ("https://example.com/a/", good.1, good.2, good.3, "from"),
            ("https://example.com/a b", good.1, good.2, good.3, "from"),
            (good.0, "../x", good.2, good.3, "into"),
            (good.0, "/x", good.2, good.3, "into"),
            (good.0, "C:/x", good.2, good.3, "into"),
            (good.0, good.1, "a/../z.bin", good.3, "z.bin"),
            // `..\..\x.bat` and `models\..\..\x`, which Windows reads as paths outside it.
            (good.0, good.1, r"..\\..\\x.bat", good.3, "x.bat"),
            (good.0, r"models\\..\\..\\x", good.2, good.3, "into"),
            (good.0, good.1, good.2, &HASH[1..], "SHA-256"),
            (good.0, good.1, good.2, &HASH.to_uppercase(), "SHA-256"),
        ];
        for (from, into, name, hash, named) in cases {
            let text = list(from, into, name, hash);
            let failure = Fetch::parse(&text).expect_err(&text);
            assert_eq!(failure.code, "fetch.bad");
            assert!(failure.message.contains(named), "{failure:?}");
        }
        // The same file from two sources.
        let one = list(good.0, good.1, good.2, good.3);
        let twice = one.replace("\n]\n", &format!("\n{}", &one[2..]));
        let failure = Fetch::parse(&twice).expect_err(&twice);
        assert!(failure.message.contains("twice"), "{failure:?}");
        let unknown = one.replace("into:", "license: \"MIT\",\n        into:");
        assert_eq!(
            Fetch::parse(&unknown).map_err(|f| f.code),
            Err("fetch.bad".to_owned())
        );
        // IDs that aren't, of what isn't an asset or isn't fetched, and one given twice.
        let model = list(good.0, good.1, "m.gltf", HASH);
        let id = |name: &str, id: &str| format!("            \"{name}\": \"{id}\",\n");
        let other = list(good.0, "models/y", "n.gltf", HASH);
        let other = with_ids(&other, &id("n.gltf", "5f3c9a1e07b2d846"));
        let both = |ids: &str| {
            let first = with_ids(&model, ids);
            first.replace("\n]\n", &format!("\n{}", &other[2..]))
        };
        for (ids, named) in [
            (id("m.gltf", "5F3C9A1E07B2D846"), "m.gltf"),
            (id("m.gltf", "0000000000000000"), "m.gltf"),
            (id("z.bin", "0a1b2c3d4e5f6071"), "z.bin"),
            (id("x.gltf", "0a1b2c3d4e5f6071"), "x.gltf"),
            (id("m.gltf", "5f3c9a1e07b2d846"), "twice"),
        ] {
            let text = both(&ids);
            let failure = Fetch::parse(&text).expect_err(&text);
            assert_eq!(failure.code, "fetch.bad");
            assert!(failure.message.contains(named), "{failure:?}");
        }
        Fetch::parse(&both(&id("m.gltf", "0a1b2c3d4e5f6071"))).expect("IDs of their own");
        let missing = Fetch::read(Path::new("no such project")).map_err(|f| f.code);
        assert_eq!(missing, Err("fetch.read".to_owned()));
    }
}
