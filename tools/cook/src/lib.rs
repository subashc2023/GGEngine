//! The offline asset cooker, as ARCHITECTURE.md's Project state says. It turns a game's project
//! into the package its builds without `dev` run from. Files that take more than reading to load
//! are made ahead, as a run would make them; textures and models are compressed and optimized to
//! load and draw faster. [`cook()`] says what the package holds.

mod mesh;
mod texture;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};

use gg_api::asset::{self, Found};
use gg_api::{Fetch, Project};
use gg_engine::cook::{self, Compressed, Kind, Lod, Texels, UiAsset, Writer};
use gg_engine::{Error, Result};

/// A file a package holds, as [`cook()`] cooked it.
#[derive(Debug, PartialEq)]
pub struct Cooked {
    /// Its path from the asset root, `/` between its parts.
    pub path: String,
    pub kind: Kind,
    /// Its asset ID, if it has one.
    pub id: Option<u64>,
    /// Its size in the package.
    pub bytes: usize,
}

/// What [`cook()`] makes of a file under the asset root, by its extension.
#[derive(Clone, Copy, PartialEq)]
enum Take {
    /// As it is.
    File,
    Texture,
    /// With the files it names.
    Model,
    Font,
    Shader,
    /// As it is, once it reads as a UI.
    Ui,
}

impl Take {
    /// What a file at `path` is taken as, by its extension in any case. `None` for what no run
    /// loads, such as code, docs, or a model's buffers, which its model holds.
    fn of(path: &str) -> Option<Self> {
        let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
        Some(match extension.as_str() {
            "wav" | "ogg" | "flac" | "mp3" | "ron" | "prefab" | "luau" => Self::File,
            "png" | "jpg" | "jpeg" | "hdr" => Self::Texture,
            "gltf" | "glb" => Self::Model,
            "ttf" | "otf" => Self::Font,
            "slang" => Self::Shader,
            "ui" => Self::Ui,
            _ => return None,
        })
    }
}

/// Cooks the project whose `project.ron` is in `dir` into the package at `out`, written whole.
/// Returns what it holds, in path order. Cooking the same project again gives the same bytes.
///
/// The package holds the files runs load under the asset root, in the folders gg-api's scan goes
/// through. It leaves out a mod's folder, holding a `mod.ron`, as mods are the player's; the
/// manifest; the icon it names, which the game's build compiles in; and the fetch list.
///
/// Textures go to BC7, or a Radiance HDR's to BC6H, of one level, as a layer's texture has. Models
/// are optimized and their images compressed, as `model` says. Fonts are baked into their fields,
/// and shaders compiled, but for a file that doesn't compile alone and another includes. The rest
/// go as they are. Each texture, model, font, prefab and UI keeps the ID its ID file holds.
///
/// Fails, writing nothing, as reading the manifest or adding a texture, model, font, UI or shader
/// fails, or with:
///
/// - `cook.unread` for a folder or file it can't read;
/// - `asset.bad_meta` or `asset.meta_case` for an ID file that doesn't read, as gg-api's scan finds
///   it, and `asset.duplicate_id` for an ID two assets' ID files hold;
/// - `asset.no_font` or `asset.no_texture` for a font or texture a UI names that a run from the
///   package wouldn't find there;
/// - `cook.uncompressed` for a texture that doesn't compress;
/// - `cook.no_start` if the start scene isn't among what it holds, as in a folder the scan skips;
/// - `package.unwritten` if the package can't be written.
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
    let (manifest, fetch) = (dir.join(Project::FILE), dir.join(Fetch::FILE));
    // The icon is the executable's and the window's, which the game's build compiles in.
    let icon = project.icon_path(dir);
    let unpacked = |file: &Path| file == manifest || file == fetch || icon.as_deref() == Some(file);
    let taken: Vec<_> = (found.iter())
        .filter(|found| !in_mod(&found.path) && !unpacked(&found.file))
        .filter_map(|found| Some((found, Take::of(&found.path)?)))
        .collect();
    let mut package = Package {
        writer: Writer::new(
            &project.assets,
            Some(&project.start),
            project.relay.as_deref(),
        ),
        cooked: BTreeMap::new(),
    };
    // Made on the job system's threads, then added as a cook one file at a time would: shaders
    // last, each kind in the scan's order, so it fails as that cook would first.
    let made = cook::in_order(taken, &|(found, take)| (found, make(found, take)));
    let mut shaders = Vec::new();
    let mut uis = Vec::new();
    for (Found { path, file }, made) in made {
        let id = ids.get(path).copied();
        match made {
            Made::Entry(kind, data) => package.add(path, id, kind, data?)?,
            Made::Shader(compiled) => shaders.push((path, file, compiled)),
            Made::Ui(read) => {
                let cook::ReadUi { bytes, named } = read?;
                package.add(path, id, Kind::File, bytes)?;
                uis.push((path, named));
            }
        }
    }
    // A file that doesn't compile alone may be a module that others include, which slangc lists
    // with the files it read.
    let mut included = BTreeSet::new();
    let mut failed = Vec::new();
    for (path, file, compiled) in shaders {
        match compiled {
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
    for (ui, named) in uis {
        for (kind, name) in &named {
            find(ui, *kind, name, &root, &ids, &package)?;
        }
    }
    // What every run loads first. Other files, which code names, the cook can't know.
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

/// What [`cook()`] makes of a file, or why it can't, to add to the package.
enum Made {
    Entry(Kind, Result<Vec<u8>>),
    Ui(Result<cook::ReadUi>),
    /// Its SPIR-V and the files slangc read, its own first.
    Shader(Result<(Vec<u8>, Vec<PathBuf>)>),
}

/// What [`cook()`] makes of `found`, taken as `take`.
fn make(found: &Found, take: Take) -> Made {
    let Found { path, file } = found;
    match take {
        Take::File => Made::Entry(Kind::File, read(file)),
        Take::Texture => {
            let ktx2 = cook::read_image(path, file).and_then(|image| {
                // A Radiance HDR's as BC6H; others' as BC7.
                let colors = match image.format {
                    cook::Format::Rgba16f => Compressed::Bc6h,
                    _ => Compressed::Bc7Srgb,
                };
                texture::compress(path, image.size, &image.pixels, false, colors)
            });
            Made::Entry(Kind::Texture, ktx2)
        }
        Take::Model => Made::Entry(Kind::Model, model(path, file)),
        Take::Font => Made::Entry(Kind::Font, cook::bake_font(path, file)),
        Take::Shader => Made::Shader(cook::compile_shader(path, file)),
        Take::Ui => Made::Ui(cook::read_ui(path, file)),
    }
}

/// Finds the font or texture, as `kind` says, that UI `ui` names `name`, in `package` as a run from
/// it would. A name is an ID, which `ids` maps paths to, or a path from `root`. A model's image is
/// named by its model and index.
///
/// Fails with `asset.no_font` or `asset.no_texture`, as the run's add of the UI would, for an ID no
/// asset has, an absolute path or one outside the root, a file the package doesn't hold or of
/// another kind, or an image the model hasn't.
fn find(
    ui: &str,
    kind: UiAsset,
    name: &str,
    root: &Path,
    ids: &HashMap<String, u64>,
    package: &Package,
) -> Result<()> {
    let (code, what, held) = match kind {
        UiAsset::Font => ("asset.no_font", "font", Kind::Font),
        UiAsset::Texture => ("asset.no_texture", "texture", Kind::Texture),
    };
    let refused = |why: String| {
        let message = format!("UI {ui} names the {what} `{name}`, which a run can't load: {why}");
        Error::new(code, message)
    };
    let image = name.rsplit_once('#').filter(|&(_, image)| {
        kind == UiAsset::Texture && !image.is_empty() && image.bytes().all(|b| b.is_ascii_digit())
    });
    let (asset, held) = match image {
        Some((model, _)) => (model, Kind::Model),
        None => (name, held),
    };
    let path = if cook::is_id(asset) {
        let id = u64::from_str_radix(asset, 16).ok();
        let found = ids.iter().find(|&(_, &has)| Some(has) == id);
        let found = found.ok_or_else(|| refused(format!("no asset has the ID {asset}")))?;
        found.0.clone()
    } else {
        let why = "a path names a file under the asset root, from it";
        cook::within(root, asset).ok_or_else(|| refused(why.to_owned()))?
    };
    match package.cooked.get(&path) {
        Some(cooked) if cooked.kind == held => {}
        Some(_) => return Err(refused(format!("{path} isn't a {what} the package holds"))),
        None => return Err(refused(format!("the package holds no {path}"))),
    }
    if let Some((_, image)) = image {
        let images = cook::read_model(&path, &root.join(&path))?.images.len();
        if !image.parse::<usize>().is_ok_and(|image| image < images) {
            return Err(refused(format!("model {path} has {images} images")));
        }
    }
    Ok(())
}

/// Model `path`'s file at `file`, cooked as a run from the package reads it.
///
/// - Each primitive's triangles are optimized and given levels of detail, keeping their order
///   where its material blends. Its vertices are packed as a run packs them, and its levels cut
///   into meshlets but where it blends or is skinned.
/// - Each image is compressed as what it holds says, BC7 of colors or values or BC5 of a normal
///   map, with the full mip chain a run makes. A DDS image, as MSFT_texture_dds names one, keeps
///   its BC7 blocks and levels.
///
/// Fails as adding the model fails, and with `cook.uncompressed` for an image that doesn't
/// compress.
fn model(path: &str, file: &Path) -> Result<Vec<u8>> {
    let mut model = cook::read_model(path, file)?;
    let compress = |(i, image): (usize, &cook::Image)| {
        let texels = cook::texels(&model, i);
        let name = format!("{path}#{i}");
        // A DDS image, which reading the model gives as blocks. BC7's blocks are the same for
        // colors or values: the texture's use picks the format.
        if let cook::Format::Bc { mipmapped, .. } = image.format {
            let compressed = match texels {
                Texels::Map(_) => Compressed::Bc7Unorm,
                _ => Compressed::Bc7Srgb,
            };
            return texture::keep(&name, image.size, &image.pixels, mipmapped, compressed);
        }
        let levels = cook::levels(&model, i);
        // Other images have their full chain of levels, which `levels` made.
        texture::compress(&name, image.size, &levels, true, Compressed::of(texels))
    };
    let images = cook::in_order(model.images.iter().enumerate().collect(), &compress);
    let images = images.into_iter().collect::<Result<Vec<_>>>()?;
    for primitive in &mut model.primitives {
        let blends = primitive.blends();
        let lods = mesh::optimize(
            mesh::Vertices {
                positions: &mut primitive.positions,
                uvs: &mut primitive.uvs,
                colors: &mut primitive.colors,
                normals: &mut primitive.normals,
                tangents: &mut primitive.tangents,
                joints: &mut primitive.joints,
                weights: &mut primitive.weights,
            },
            &mut primitive.indices,
            blends,
        );
        let levels = (lods.into_iter())
            .map(|lod| Lod {
                start: lod.start,
                count: lod.count,
                error: lod.error,
            })
            .collect();
        let mut packed = cook::pack(path, primitive, levels)?;
        // What blends draws its triangles in order, through its indices. What's skinned does too:
        // its joints would move it out of its meshlets' bounds.
        if !blends && packed.skin.is_empty() {
            mesh::cut(&mut packed);
        }
        primitive.mesh = Some(packed);
    }
    cook::model_bytes(path, &model, &images)
}

/// A package as [`cook()`] makes it, and what it holds so far.
struct Package {
    writer: Writer,
    /// By their paths.
    cooked: BTreeMap<String, Cooked>,
}

impl Package {
    /// Adds `data` as the file at `path` from the asset root. Fails as [`Writer::add`] does.
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

/// Each asset's ID under `root`, by its path from the root, as its ID file holds it.
///
/// Fails with `asset.bad_meta` or `asset.meta_case` for an ID file that doesn't read, and
/// `asset.duplicate_id` for an ID two hold.
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

/// Fails with `cook.unread` if `file` can't be read.
fn read(file: &Path) -> Result<Vec<u8>> {
    fs::read(file).map_err(|error| {
        let message = format!("can't read {}", file.display());
        Error::new("cook.unread", message).with_source(error)
    })
}

/// `file`'s path from `root`, as written, `/` between parts, as the package names it and a run
/// asks for it. `None` outside the root or through `..`.
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

    /// A UI of a button, and of the stone texture by its ID and by its path.
    const MENU: &str = "(nodes: [Button(name: \"play\"), Image(texture: \"5f3c9a1e07b2d846\"), \
                        Panel(texture: \"textures/./stone.png\")])";

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

    /// A test's project in the OS's temporary directory, holding `files`, paths from it and their
    /// text. Removed as it drops.
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

    impl Fixture {
        /// Writes `bytes` at `path` from its directory.
        fn write(&self, path: &str, bytes: &[u8]) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A PNG `size` texels across and down, every one `texel`.
    fn png(size: [u32; 2], texel: [u8; 4]) -> Vec<u8> {
        let mut file = Vec::new();
        let mut encoder = png::Encoder::new(&mut file, size[0], size[1]);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let texels = texel.repeat(size[0] as usize * size[1] as usize);
        writer.write_image_data(&texels).unwrap();
        writer.finish().unwrap();
        file
    }

    /// The cooker's test model `name`.
    fn test_model(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/models")
            .join(name)
    }

    fn id_file(id: &str) -> String {
        Meta {
            id: id.parse::<AssetId>().unwrap(),
        }
        .text()
    }

    /// A package holds what runs load, with their IDs: a compressed texture, a cooked model, a
    /// scene, prefab, UI and script as they are, and a compiled shader but not the module it
    /// includes. It leaves out the manifest, though not its relay; the icon and its ID file; the
    /// fetch list; a mod's folder; and code and notes. Cooked again, it's the same bytes.
    #[test]
    fn a_package_holds_what_runs_load() {
        let stone = id_file("5f3c9a1e07b2d846");
        let lamp = id_file("6a4d0b2f18c9e357");
        let menu = id_file("7b5e1c3a29d0f468");
        let icon = id_file("8c6f2d4b3ae1f579");
        let relay = "relay.example.com:7800";
        let project = PROJECT.replace(
            ")\n",
            &format!("    relay: Some(\"{relay}\"),\n    icon: Some(\"art/icon.png\"),\n)\n"),
        );
        let fixture = Fixture::new(
            "holds",
            &[
                ("project.ron", &project),
                ("fetch.ron", "[]\n"),
                ("scenes/main.ron", SCENE),
                ("prefabs/lamp.prefab", SCENE),
                ("prefabs/lamp.prefab.meta", &lamp),
                ("ui/menu.ui", MENU),
                ("ui/menu.ui.meta", &menu),
                ("textures/stone.png.meta", &stone),
                ("art/icon.png.meta", &icon),
                ("shaders/flat.slang", FLAT),
                ("shaders/common.slang", COMMON),
                ("scripts/spin.luau", "return nil\n"),
                ("mods/rainbow/mod.ron", "()"),
                ("mods/rainbow/rainbow.luau", "return nil\n"),
                ("src/main.rs", "fn main() {}\n"),
                ("notes.md", "# Notes\n"),
            ],
        );
        fixture.write("textures/stone.png", &png([5, 3], [90, 80, 70, 255]));
        fixture.write("art/icon.png", &png([256, 256], [200, 120, 40, 255]));
        let crate_model = fs::read(test_model("crate.gltf")).unwrap();
        fixture.write("models/crate.gltf", &crate_model);
        let out = fixture.0.join("target/game.ggpk");
        let cooked = cook(&fixture.0, &out).unwrap_or_else(|error| panic!("{error:#}"));
        let held: Vec<(&str, Kind, Option<u64>)> = (cooked.iter())
            .map(|cooked| (&*cooked.path, cooked.kind, cooked.id))
            .collect();
        let expected = [
            ("models/crate.gltf", Kind::Model, None),
            (
                "prefabs/lamp.prefab",
                Kind::File,
                Some(0x6a4d_0b2f_18c9_e357),
            ),
            ("scenes/main.ron", Kind::File, None),
            ("scripts/spin.luau", Kind::File, None),
            ("shaders/flat.slang", Kind::Shader, None),
            (
                "textures/stone.png",
                Kind::Texture,
                Some(0x5f3c_9a1e_07b2_d846),
            ),
            ("ui/menu.ui", Kind::File, Some(0x7b5e_1c3a_29d0_f468)),
        ];
        assert_eq!(held, expected);
        let first = fs::read(&out).unwrap();
        // In its header's strings, which gg-engine's tests read back.
        assert!(
            first
                .windows(relay.len())
                .any(|bytes| bytes == relay.as_bytes()),
            "the relay"
        );
        cook(&fixture.0, &out).unwrap_or_else(|error| panic!("{error:#}"));
        assert_eq!(fs::read(&out).unwrap(), first, "the same bytes");
    }

    /// A scene naming a built-in shape cooks as it is, the package holding no file for the shape,
    /// which every run makes itself.
    #[test]
    fn a_scene_naming_a_shape_cooks_without_it() {
        let scene = SCENE.replace(
            "{}",
            "{\n        \"0000000000000100\": (\n            ModelRoot: (\"gg:cube\"),\n        ),\n    }",
        );
        let files = [("project.ron", PROJECT), ("scenes/main.ron", &*scene)];
        let fixture = Fixture::new("shape", &files);
        let out = fixture.0.join("target/game.ggpk");
        let cooked = cook(&fixture.0, &out).unwrap_or_else(|error| panic!("{error:#}"));
        let held: Vec<&str> = cooked.iter().map(|cooked| &*cooked.path).collect();
        assert_eq!(held, ["scenes/main.ron"]);
    }

    /// A model cooks to what a run reads: its primitives, each with levels of detail, the first its
    /// own triangles, and its images, compressed as what they hold, with full mip chains.
    #[test]
    fn a_model_cooks_to_what_a_run_reads() {
        for name in ["crate.gltf", "torus.gltf", "tiles.gltf", "fox/Fox.gltf"] {
            let file = test_model(name);
            let read = cook::read_model(name, &file).unwrap_or_else(|error| panic!("{error:#}"));
            let bytes = model(name, &file).unwrap_or_else(|error| panic!("{error:#}"));
            let cooked = cook::read_cooked_model(name, &bytes)
                .unwrap_or_else(|error| panic!("{name}: {error:#}"));
            assert_eq!(cooked.primitives.len(), read.primitives.len(), "{name}");
            for (cooked, read) in cooked.primitives.iter().zip(&read.primitives) {
                let mesh = cooked.mesh.as_ref().expect("a cooked mesh");
                let finest = mesh.lods.levels.first().expect("a level");
                assert_eq!(finest.start, 0, "{name}");
                assert_eq!(finest.count as usize, read.indices.len(), "{name}");
                assert_eq!(finest.error, 0.0, "{name}");
                // Cut into meshlets but where it blends or is skinned, which keeps a weight a
                // vertex.
                let levels = mesh.meshlets.levels.len();
                let skinned = !read.joints.is_empty();
                let cut = if read.blends() || skinned {
                    0
                } else {
                    mesh.lods.levels.len()
                };
                assert_eq!(levels, cut, "{name}");
                let weights = if skinned { mesh.vertices.len() } else { 0 };
                assert_eq!(mesh.skin.len(), weights, "{name}");
            }
            assert_eq!(cooked.images.len(), read.images.len(), "{name}");
            for (i, (cooked, image)) in cooked.images.iter().zip(&read.images).enumerate() {
                assert_eq!(cooked.size, image.size, "{name}");
                let format = cook::Format::Bc {
                    compressed: Compressed::of(cook::texels(&read, i)),
                    mipmapped: true,
                };
                assert_eq!(cooked.format, format, "{name}'s image {i}");
            }
        }
        // The torus is enough triangles to simplify.
        let torus = test_model("torus.gltf");
        let bytes = model("torus.gltf", &torus).unwrap();
        let cooked = cook::read_cooked_model("torus.gltf", &bytes).unwrap();
        let lods = &cooked.primitives[0]
            .mesh
            .as_ref()
            .expect("a mesh")
            .lods
            .levels;
        assert!(lods.len() > 1, "{lods:?}");
    }

    /// A BC7 DDS file of `size` texels and DXGI format `format`, 98 of values or 99 of sRGB,
    /// holding `levels`' blocks, laid out as Microsoft's guide says.
    fn dds(format: u32, size: [u32; 2], levels: &[Vec<u8>]) -> Vec<u8> {
        let mut words = [0u32; 31];
        // Its header's size; DDSD_CAPS, HEIGHT, WIDTH, PIXELFORMAT and MIPMAPCOUNT; its size and
        // levels; its pixel format's size, DDPF_FOURCC, and the four characters of a DX10 header.
        words[0] = 124;
        words[1] = 0x2_1007;
        [words[2], words[3]] = [size[1], size[0]];
        words[6] = levels.len() as u32;
        words[18] = 32;
        words[19] = 0x4;
        words[20] = u32::from_le_bytes(*b"DX10");
        // The DX10 header: its DXGI format, a 2D texture, not a cube, one of them.
        let dx10 = [format, 3, 0, 1, 0];
        let words = words
            .iter()
            .chain(&dx10)
            .flat_map(|word| word.to_le_bytes());
        [b"DDS ".to_vec(), words.collect(), levels.concat()].concat()
    }

    /// A KHR_materials_pbrSpecularGlossiness model whose textures are MSFT_texture_dds images keeps
    /// their blocks and levels. Its diffuse and specular-glossiness maps become sRGB BC7, its
    /// normal map BC7 of values, whatever each file's own format says.
    #[test]
    fn a_models_dds_images_keep_their_blocks() {
        let floats = |floats: &[f32]| floats.iter().flat_map(|f| f.to_le_bytes()).collect();
        let positions: Vec<u8> = floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.]);
        let uvs: Vec<u8> = floats(&[0., 0., 1., 0., 0., 1.]);
        let bin = [positions, uvs, vec![0, 0, 1, 0, 2, 0, 0, 0]].concat();
        // 8 texels a side: levels of 4 blocks, then 1 each for 4, 2 and 1 texels.
        let levels = |byte: u8| [vec![byte; 64], vec![byte + 1; 16], vec![2; 16], vec![3; 16]];
        let gltf = format!(
            r#"{{
    "asset": {{"version": "2.0"}},
    "extensionsUsed": ["KHR_materials_pbrSpecularGlossiness", "MSFT_texture_dds"],
    "scenes": [{{"nodes": [0]}}],
    "nodes": [{{"mesh": 0}}],
    "meshes": [{{"primitives": [{{
        "attributes": {{"POSITION": 0, "TEXCOORD_0": 1}}, "indices": 2, "material": 0
    }}]}}],
    "accessors": [
        {{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"}},
        {{"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2"}},
        {{"bufferView": 2, "componentType": 5123, "count": 3, "type": "SCALAR"}}
    ],
    "materials": [{{
        "normalTexture": {{"index": 2}},
        "extensions": {{"KHR_materials_pbrSpecularGlossiness": {{
            "diffuseTexture": {{"index": 0}},
            "specularGlossinessTexture": {{"index": 1}},
            "glossinessFactor": 0.5
        }}}}
    }}],
    "textures": [
        {{"extensions": {{"MSFT_texture_dds": {{"source": 0}}}}}},
        {{"extensions": {{"MSFT_texture_dds": {{"source": 1}}}}}},
        {{"extensions": {{"MSFT_texture_dds": {{"source": 2}}}}}}
    ],
    "images": [{{"uri": "diffuse.dds"}}, {{"uri": "glossy.dds"}}, {{"uri": "normals.dds"}}],
    "buffers": [{{"uri": "square.bin", "byteLength": {}}}],
    "bufferViews": [
        {{"buffer": 0, "byteOffset": 0, "byteLength": 36}},
        {{"buffer": 0, "byteOffset": 36, "byteLength": 24}},
        {{"buffer": 0, "byteOffset": 60, "byteLength": 6}}
    ]
}}"#,
            bin.len()
        );
        let fixture = Fixture::new("dds", &[("square.gltf", &gltf)]);
        fixture.write("square.bin", &bin);
        // Of values, sRGB, and sRGB: the formats sampling them say.
        let files = [
            ("diffuse.dds", 98, 10),
            ("glossy.dds", 99, 20),
            ("normals.dds", 99, 30),
        ];
        for (name, format, byte) in files {
            fixture.write(name, &dds(format, [8, 8], &levels(byte)));
        }
        let file = fixture.0.join("square.gltf");
        let bytes = model("square.gltf", &file).unwrap_or_else(|error| panic!("{error:#}"));
        let cooked = cook::read_cooked_model("square.gltf", &bytes)
            .unwrap_or_else(|error| panic!("{error:#}"));
        let held: Vec<_> = (cooked.images.iter())
            .map(|image| (image.size, image.format, image.pixels.clone()))
            .collect();
        let blocks = |compressed, byte| {
            let format = cook::Format::Bc {
                compressed,
                mipmapped: true,
            };
            ([8, 8], format, levels(byte).concat())
        };
        let expected = [
            blocks(Compressed::Bc7Srgb, 10),
            blocks(Compressed::Bc7Srgb, 20),
            blocks(Compressed::Bc7Unorm, 30),
        ];
        assert_eq!(held, expected);
    }

    /// A project with clashing or unreadable IDs, an asset that doesn't load, a UI a run would
    /// refuse, or a start scene outside the scan, is refused, and no package is written.
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
        refused("undecoded", &[("a.png", "not a PNG")], "engine.bad_texture");
        refused("unread_model", &[("a.gltf", "{}")], "engine.bad_model");
        let button = "(nodes: [Button(text: \"PLAY\")])";
        refused("unnamed", &[("menu.ui", button)], "ui.unnamed_button");
        // A UI naming what a run from the package couldn't load: a texture it doesn't hold, one
        // outside the root, a scene for a texture, and a font by an ID no asset has.
        for (name, ui, code) in [
            (
                "no_texture",
                "Image(texture: \"ui/none.png\")",
                "asset.no_texture",
            ),
            (
                "outside",
                "Image(texture: \"../stone.png\")",
                "asset.no_texture",
            ),
            (
                "not_texture",
                "Image(texture: \"scenes/main.ron\")",
                "asset.no_texture",
            ),
            (
                "no_font",
                "Text(font: \"0123456789abcdef\")",
                "asset.no_font",
            ),
        ] {
            let ui = format!("(nodes: [{ui}])");
            refused(name, &[("menu.ui", &ui)], code);
        }
        // A start scene where the scan doesn't look, as in `tests` at the root.
        let project = PROJECT.replace("\"scenes", "\"tests");
        let files = [("project.ron", &*project), ("tests/main.ron", SCENE)];
        refused("nostart", &files, "cook.no_start");
    }

    /// A model a run refuses, a triangle spanning -f32::MAX to 0, past packed vertices' reach, is
    /// refused as a run refuses it, naming its mesh, and no package is written.
    #[test]
    fn a_model_a_run_refuses_is_refused() {
        let floats =
            |floats: &[f32]| -> Vec<u8> { floats.iter().flat_map(|f| f.to_le_bytes()).collect() };
        let bin = floats(&[-f32::MAX, 0., 0., 0., 0., 0., 0., 1., 0.]);
        let gltf = r#"{
    "asset": {"version": "2.0"},
    "scenes": [{"nodes": [0]}],
    "nodes": [{"mesh": 0}],
    "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
    "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"}],
    "buffers": [{"uri": "far.bin", "byteLength": 36}],
    "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 36}]
}"#;
        let files = [
            ("project.ron", PROJECT),
            ("scenes/main.ron", SCENE),
            ("far.gltf", gltf),
        ];
        let fixture = Fixture::new("far", &files);
        fixture.write("far.bin", &bin);
        let out = fixture.0.join("game.ggpk");
        let error = cook(&fixture.0, &out).expect_err("a model a run refuses");
        assert_eq!(error.code(), "engine.bad_model", "{error:#}");
        assert!(
            error.to_string().contains("mesh \"far.gltf 0/0\""),
            "{error:#}"
        );
        assert!(!out.exists());
    }

    /// Checks that fixture `name` of `files`, with a manifest and start scene unless given, is
    /// refused with `code`, and no package written.
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
