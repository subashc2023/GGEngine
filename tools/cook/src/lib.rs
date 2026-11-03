//! The offline asset cooker, as ARCHITECTURE.md's Project state says: a game's project into the
//! package its builds without `dev` run from, of the files under its asset root that its runs load,
//! each as the engine makes it as it loads it where that takes more than reading it, a font baked
//! into its field and a shader compiled to SPIR-V, and the rest as they are.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path};

use gg_api::Project;
use gg_api::asset::{self, Found};
use gg_engine::cook::{self, Kind, Writer};
use gg_engine::{Error, Result};

/// A file a package holds, as [`cook()`] cooked it.
#[derive(Debug, PartialEq)]
pub struct Cooked {
    /// Its path from the asset root, `/` between its parts.
    pub path: String,
    pub kind: Kind,
    /// Its asset ID, if it has one.
    pub id: Option<u64>,
    /// How many bytes the package holds of it.
    pub bytes: usize,
}

/// What [`cook()`] makes of a file under the asset root, by its extension.
#[derive(Clone, Copy, PartialEq)]
enum Take {
    /// As it is: a texture, a sound or music, a scene or other RON, or a script.
    File,
    /// As it is, with the files it names.
    Model,
    Font,
    Shader,
}

impl Take {
    /// What a file at `path` is taken as, by its extension, whatever its case; `None` for one that
    /// no run loads, which the package leaves out, as a game's code or docs.
    fn of(path: &str) -> Option<Self> {
        let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
        Some(match extension.as_str() {
            "png" | "jpg" | "jpeg" | "wav" | "ogg" | "flac" | "mp3" | "ron" | "luau" => Self::File,
            "gltf" | "glb" => Self::Model,
            "ttf" | "otf" => Self::Font,
            "slang" => Self::Shader,
            _ => return None,
        })
    }
}

/// Cooks the project whose manifest, `project.ron`, is in `dir` into the package at `out`, written
/// whole, and returns what it holds, in the order of their paths.
///
/// The package holds the files under the asset root, through the folders gg-api's scan goes
/// through, but those in a mod's folder, which holds a `mod.ron`, as mods are the player's, and the
/// manifest: textures, sounds and music, scenes and other RON, and scripts, as they are; models as
/// they are, with the buffers and images they name; fonts baked into their fields; and shaders
/// compiled, but for a file that doesn't compile alone that another includes. Each texture, model
/// and font whose ID file holds its ID has it there.
///
/// Fails, writing nothing, as the manifest's reading does; with `cook.unread` for a folder or a
/// file it can't read; with `asset.bad_meta` or `asset.meta_case` for an asset whose ID file doesn't
/// read, as gg-api's scan finds it, and `asset.duplicate_id` for an ID two assets' ID files hold;
/// with `asset.outside_root` for a model naming a file outside the asset root; as adding a model,
/// a font or a shader fails; with `cook.no_start` if the start scene isn't among what it holds, as
/// in a folder the scan doesn't go through; and with `package.unwritten` if the package can't be
/// written.
pub fn cook(dir: &Path, out: &Path) -> Result<Vec<Cooked>> {
    let project = Project::read(dir).map_err(|failure| {
        let code = match failure.code.as_str() {
            "project.read" => "project.read",
            _ => "project.bad",
        };
        Error::new(code, failure.message)
    })?;
    let root = project.assets_dir(dir);
    let (found, unread) = asset::files(&root);
    if let Some(unread) = unread.first() {
        let message = format!(
            "can't read `{}` under the asset root, {}: {}",
            unread.path,
            root.display(),
            unread.error
        );
        return Err(Error::new("cook.unread", message));
    }
    let ids = ids(&root)?;
    let mods: Vec<&str> = (found.iter())
        .filter_map(|found| found.path.strip_suffix("/mod.ron"))
        .collect();
    let in_mod = |path: &str| {
        (mods.iter()).any(|dir| {
            path.strip_prefix(dir)
                .is_some_and(|rest| rest.starts_with('/'))
        })
    };
    let manifest = dir.join(Project::FILE);
    let taken = (found.iter())
        .filter(|found| !in_mod(&found.path) && found.file != manifest)
        .filter_map(|found| Some((found, Take::of(&found.path)?)));
    let mut package = Package {
        writer: Writer::new(&project.assets, Some(&project.start)),
        cooked: BTreeMap::new(),
    };
    let (mut models, mut shaders) = (Vec::new(), Vec::new());
    for (found, take) in taken {
        let Found { path, file } = found;
        let id = ids.get(path).copied();
        match take {
            Take::File => package.add(path, id, Kind::File, read(file)?)?,
            Take::Model => {
                package.add(path, id, Kind::File, read(file)?)?;
                models.push((path, file));
            }
            Take::Font => package.add(path, id, Kind::Font, cook::bake_font(path, file)?)?,
            Take::Shader => shaders.push((path, file)),
        }
    }
    // A model's buffers and images, by their paths as the model names them, which a run reads
    // them by; a texture with an ID may be one, which the package holds once, with its ID.
    for (path, file) in models {
        for named in cook::model_files(path, file)?.into_iter().skip(1) {
            let within = within(&root, &named).ok_or_else(|| {
                let message = format!(
                    "model `{path}` names {}, outside the asset root, {}",
                    named.display(),
                    root.display()
                );
                Error::new("asset.outside_root", message)
            })?;
            if !package.cooked.contains_key(&within) {
                package.add(&within, None, Kind::File, read(&named)?)?;
            }
        }
    }
    // A file that doesn't compile alone may be a module that others include, which slangc lists
    // with the files it read.
    let mut included = BTreeSet::new();
    let mut failed = Vec::new();
    for (path, file) in shaders {
        match cook::compile_shader(path, file) {
            Ok((spirv, read)) => {
                let read = read.iter().skip(1);
                included.extend(read.filter_map(|file| fs::canonicalize(file).ok()));
                package.add(path, None, Kind::Shader, spirv)?;
            }
            Err(error) => failed.push((file, error)),
        }
    }
    if let Some((_, error)) = (failed.into_iter())
        .find(|(file, _)| fs::canonicalize(file).is_ok_and(|file| !included.contains(&file)))
    {
        return Err(error);
    }
    // What every run loads first: its other files, which its code names, the cook can't know.
    let start = project.start_path(dir);
    if !within(&root, &start).is_some_and(|start| package.cooked.contains_key(&start)) {
        let message = format!(
            "the start scene, {}, isn't under the asset root, {}, where the cook looks",
            start.display(),
            root.display()
        );
        return Err(Error::new("cook.no_start", message));
    }
    package.writer.write(out)?;
    Ok(package.cooked.into_values().collect())
}

/// A package as [`cook()`] makes it, and what it holds so far.
struct Package {
    writer: Writer,
    /// By their paths.
    cooked: BTreeMap<String, Cooked>,
}

impl Package {
    /// Adds the file at `path` from the asset root, of `kind`, as `data`, with the ID `id` names.
    ///
    /// Fails as [`Writer::add`] does.
    fn add(&mut self, path: &str, id: Option<u64>, kind: Kind, data: Vec<u8>) -> Result<()> {
        let bytes = data.len();
        self.writer.add(path, id, kind, data)?;
        let path = path.to_owned();
        let cooked = Cooked {
            path: path.clone(),
            kind,
            id,
            bytes,
        };
        self.cooked.insert(path, cooked);
        Ok(())
    }
}

/// The ID each asset under `root` has, by its path from the root, as its ID file holds it.
///
/// Fails with `asset.bad_meta` or `asset.meta_case` for an ID file that doesn't read, as gg-api's
/// scan finds it, and with `asset.duplicate_id` for an ID two hold.
fn ids(root: &Path) -> Result<HashMap<String, u64>> {
    let mut ids = HashMap::new();
    let mut holders: HashMap<u64, String> = HashMap::new();
    for scanned in asset::scan(root).assets {
        match scanned.meta {
            None => {}
            Some(Ok(meta)) => {
                let id = meta.id.get();
                if let Some(other) = holders.insert(id, scanned.path.clone()) {
                    let message = format!(
                        "`{other}` and `{}` both have the ID {}: a package names an asset by its \
                         ID, so one's ID file must go, for a run to write it a fresh one",
                        scanned.path, meta.id
                    );
                    return Err(Error::new("asset.duplicate_id", message));
                }
                ids.insert(scanned.path, id);
            }
            Some(Err(failure)) => {
                let code = match failure.code.as_str() {
                    "asset.meta_case" => "asset.meta_case",
                    _ => "asset.bad_meta",
                };
                let message = format!("`{}`'s ID file: {}", scanned.path, failure.message);
                return Err(Error::new(code, message));
            }
        }
    }
    Ok(ids)
}

/// The file at `file`, whole.
///
/// Fails with `cook.unread` if it can't be read.
fn read(file: &Path) -> Result<Vec<u8>> {
    fs::read(file).map_err(|error| {
        let message = format!("can't read {}", file.display());
        Error::new("cook.unread", message).with_source(error)
    })
}

/// The path from `root` of `file`, as written, `/` between its parts, as the package names it and
/// a run asks for it; `None` for a file outside it, or a path through `..`, which a model's URI
/// can't have.
fn within(root: &Path, file: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for part in file.strip_prefix(root).ok()?.components() {
        match part {
            Component::Normal(part) => parts.push(part.to_str()?),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gg_api::asset::{AssetId, Meta};

    use super::*;

    const PROJECT: &str = "(\n    game: \"fixture\",\n    assets: \".\",\n    scenes: [\n        \
                           \"scenes\",\n    ],\n    start: \"scenes/main.ron\",\n)\n";

    /// A scene without entities.
    const SCENE: &str = "(\n    entities: {},\n)\n";

    /// A module that a shader includes, which doesn't compile alone.
    const COMMON: &str = "float4 red() { return float4(1.0, 0.0, 0.0, 1.0); }\n";

    const FLAT: &str = r#"#include "common.slang"
struct Vertex { float3 position; };
struct Push {
    float4x4 clip_from_world;
    Vertex* vertices;
    float4x4* transforms;
    uint2* ids;
    float4* params;
};
[[vk::push_constant]] ConstantBuffer<Push> push;
struct Varyings { float4 position : SV_Position; nointerpolation uint2 id; };
[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    float4 world = mul(push.transforms[instance], float4(push.vertices[index].position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    out.id = push.ids[instance];
    return out;
}
struct Targets { float4 color : SV_Target0; uint2 id : SV_Target1; };
[shader("fragment")]
Targets fragment_main(Varyings input) {
    Targets out;
    out.color = red();
    out.id = input.id;
    return out;
}
"#;

    /// A project of a test's own, in the OS's temporary directory, holding `files`, each a path
    /// from its directory and what it holds; removed as it drops.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new(name: &str, files: &[(&str, &str)]) -> Self {
            let dir = std::env::temp_dir().join(format!("gg-cook-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            for (path, text) in files {
                let path = dir.join(path);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, text).unwrap();
            }
            Self(dir)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn id_file(id: &str) -> String {
        Meta {
            id: id.parse::<AssetId>().unwrap(),
        }
        .text()
    }

    /// A package holds what a game's runs load, by their paths from the asset root: a texture as
    /// it is, with the ID its ID file holds; a scene and a script as they are; and a shader
    /// compiled, but the module it includes, which doesn't compile alone; and leaves out the
    /// manifest, a mod's folder, and what no run loads, as code and notes. Cooked again, it's the
    /// same bytes.
    #[test]
    fn a_package_holds_what_runs_load() {
        let stone = id_file("5f3c9a1e07b2d846");
        let fixture = Fixture::new(
            "holds",
            &[
                ("project.ron", PROJECT),
                ("scenes/main.ron", SCENE),
                ("textures/stone.png", "not decoded as it's cooked"),
                ("textures/stone.png.meta", &stone),
                ("shaders/flat.slang", FLAT),
                ("shaders/common.slang", COMMON),
                ("scripts/spin.luau", "return nil\n"),
                ("mods/rainbow/mod.ron", "()"),
                ("mods/rainbow/rainbow.luau", "return nil\n"),
                ("src/main.rs", "fn main() {}\n"),
                ("notes.md", "# Notes\n"),
            ],
        );
        let out = fixture.0.join("target/game.ggpk");
        let cooked = cook(&fixture.0, &out).unwrap_or_else(|error| panic!("{error:#}"));
        let held: Vec<(&str, Kind, Option<u64>)> = (cooked.iter())
            .map(|cooked| (&*cooked.path, cooked.kind, cooked.id))
            .collect();
        let expected = [
            ("scenes/main.ron", Kind::File, None),
            ("scripts/spin.luau", Kind::File, None),
            ("shaders/flat.slang", Kind::Shader, None),
            (
                "textures/stone.png",
                Kind::File,
                Some(0x5f3c_9a1e_07b2_d846),
            ),
        ];
        assert_eq!(held, expected);
        assert_eq!(cooked[3].bytes, "not decoded as it's cooked".len());
        let first = fs::read(&out).unwrap();
        cook(&fixture.0, &out).unwrap_or_else(|error| panic!("{error:#}"));
        assert_eq!(fs::read(&out).unwrap(), first, "the same bytes");
    }

    /// A project whose package couldn't name its assets alike, or whose shader doesn't compile, is
    /// refused, and no package is written.
    #[test]
    fn a_project_that_wouldnt_run_alike_is_refused() {
        let twice = id_file("5f3c9a1e07b2d846");
        let files = [
            ("a.png", ""),
            ("a.png.meta", &twice),
            ("b.png", ""),
            ("b.png.meta", &twice),
        ];
        refused("twice", &files, "asset.duplicate_id");
        let files = [("a.png", ""), ("a.png.meta", "(id: \"5f3c")];
        refused("unread", &files, "asset.bad_meta");
        refused(
            "uncompiled",
            &[("broken.slang", "not a shader")],
            "renderer.bad_shader",
        );
        // A start scene where the scan doesn't look, as in `tests` at the root.
        let project = PROJECT.replace("\"scenes", "\"tests");
        let files = [("project.ron", &*project), ("tests/main.ron", SCENE)];
        refused("nostart", &files, "cook.no_start");
    }

    /// Checks that a project of `files`, beside its manifest and start scene unless they're among
    /// them, a fixture named `name`, is refused with `code`, and no package written.
    fn refused(name: &str, files: &[(&str, &str)], code: &str) {
        let mut files = files.to_vec();
        for (path, text) in [("project.ron", PROJECT), ("scenes/main.ron", SCENE)] {
            if !files.iter().any(|(given, _)| *given == path) {
                files.push((path, text));
            }
        }
        let fixture = Fixture::new(name, &files);
        let out = fixture.0.join("game.ggpk");
        let error = cook(&fixture.0, &out).expect_err(name);
        assert_eq!(error.code(), code, "{name}: {error:#}");
        assert!(!out.exists(), "{name}");
    }
}
