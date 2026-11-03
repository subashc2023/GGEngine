//! `gg ship`: a game built as it ships, without `dev` in the shipping profile, beside the package
//! the workspace's cooker cooks from its project, laid out in a folder of its own: on Windows and
//! Linux, whose GPUs' drivers install the Vulkan loader, the executable and its package side by
//! side; on macOS, an app bundle holding them with the Vulkan SDK's loader and KosmicKrisp, its
//! Vulkan driver over Metal, signed ad hoc.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use gg_api::{Failure, Project};
use serde_json::{Value, json};

use crate::{Scope, build, run};

/// What `gg ship` ships: a package, a project's game, and the project gg works in, if any.
pub(crate) struct Ship {
    pub(crate) package: String,
    pub(crate) project: Option<Scope>,
}

/// Cargo's flags for a build as it ships: without the game's `dev`, in the shipping profile, which
/// the root manifest gives.
const SHIPPING: [&str; 3] = ["--no-default-features", "--profile", "shipping"];

/// The loader's name in a bundle's `Contents/Frameworks`, which gg-gpu loads it by, and the link
/// to its file in the Vulkan SDK.
const LOADER: &str = "libvulkan.1.dylib";

/// KosmicKrisp's library, and its manifest, by the Vulkan SDK's names, which a bundle keeps.
const DRIVER: &str = "libvulkan_kosmickrisp.dylib";
const DRIVER_MANIFEST: &str = "libkosmickrisp_icd.json";

/// The oldest loader a bundle holds, the Vulkan SDK 1.4.363.0's: the first SDK whose KosmicKrisp is
/// a conformant Vulkan 1.4 driver, and whose loader honors `VK_LOADER_SEARCH_ONLY_IN_BUNDLE`, which
/// the bundle's Info.plist sets.
const OLDEST_LOADER: [u32; 3] = [1, 4, 363];

/// Builds `ship`'s package as it ships, cooks its project into its package beside its executable,
/// where a build without `dev` finds it, and lays the two out in `ship/<package>` in the target
/// directory, emptied first, as [`files`] says; on macOS, with the loader and KosmicKrisp of the
/// Vulkan SDK that `VULKAN_SDK` names, signed as [`sign`] says. Prints a line of JSON naming the
/// folder and each file in it, with its size.
///
/// Fails as [`run::check_game`] does; with `project.read` or `project.bad` for a package whose
/// directory holds no project that reads, and `gg.project` for one whose project names another
/// game; on macOS, before building, as [`sdk`] does; with `gg.build` if the workspace has no such
/// package, it has no binary or it doesn't build, `gg.cook` if its project doesn't cook, and
/// `gg.cargo` if cargo can't run to say what the workspace holds; with `gg.ship` if the folder
/// can't be laid out; and as `sign` does. A folder that fails to be laid out or signed is removed,
/// so that none looks shipped that isn't.
pub(crate) fn ship(ship: &Ship) -> Result<(), Failure> {
    let dir = ship.project.as_ref().map(Scope::dir);
    run::check_game(&ship.package, ship.project.as_ref())?;
    let package = build::package(&ship.package, dir)?;
    let project = package.manifest.parent().unwrap_or(Path::new("."));
    let game = Project::read(project)?.game;
    if game != ship.package {
        let message = format!(
            "the project in {} names its game `{game}`, not `{}`, beside whose manifest it is",
            project.display(),
            ship.package
        );
        return Err(Failure::new("gg.project", message));
    }
    // Before the build, which takes minutes.
    let sdk = if cfg!(target_os = "macos") {
        Some(sdk(std::env::var_os("VULKAN_SDK"))?)
    } else {
        None
    };
    let exe = build::build(&ship.package, &SHIPPING, dir)?;
    let ggpk = exe.with_extension("ggpk");
    cook(project, &ggpk, dir)?;
    let out = package.target.join("ship").join(&ship.package);
    let files = files(&ship.package, &package.version, &exe, &ggpk, sdk.as_ref());
    let shipped = lay_out(&out, &files).and_then(|laid| {
        if sdk.is_some() {
            let libraries: Vec<PathBuf> = (files.iter())
                .filter(|(path, _)| path.ends_with(".dylib"))
                .map(|(path, _)| at(&out, path))
                .collect();
            sign(&out.join(bundle(&ship.package)), &libraries)?;
        }
        Ok(laid)
    });
    if shipped.is_err() {
        let _ = fs::remove_dir_all(&out);
    }
    let laid = shipped?;
    crate::print(&json!({"shipped": ship.package, "dir": out, "files": laid}).to_string());
    Ok(())
}

/// Cooks the project in directory `project` into the package at `ggpk`, with the workspace's
/// cooker, through cargo run in `dir`; the lines the cooker prints go to stderr, as gg prints one
/// of its own.
///
/// Fails with `gg.cook` if cargo can't run, or the cooker fails, as it said on stderr.
fn cook(project: &Path, ggpk: &Path, dir: Option<&Path>) -> Result<(), Failure> {
    let failed = |what: String| Failure::new("gg.cook", what);
    let status = (build::cargo(dir).args(["run", "--package", "cook", "--"]))
        .args([project, ggpk])
        .stdout(io::stderr())
        .status()
        .map_err(|error| failed(format!("can't run cargo: {error}")))?;
    if !status.success() {
        let message = format!(
            "cooking {} into {} failed, as the cooker said above",
            project.display(),
            ggpk.display()
        );
        return Err(failed(message));
    }
    Ok(())
}

/// What a file of a shipped folder holds: a copy of a file, or text.
#[derive(Debug, PartialEq)]
enum Content {
    Copy(PathBuf),
    Text(String),
}

/// The files of game `package`'s shipped folder, by their paths in it, `/` between their parts:
/// its executable, `exe`, and its package, `ggpk`, side by side; or, with the Vulkan SDK's `sdk`,
/// as on macOS, an app bundle holding them, named as [`bundle`] says, as Apple lays bundles out,
/// the executable in `Contents/MacOS` and the package, which is data, in `Contents/Resources`,
/// where an executable in a bundle finds it; with the SDK's loader and KosmicKrisp in
/// `Contents/Frameworks`, the manifest by which the loader finds KosmicKrisp in
/// `Contents/Resources/vulkan/icd.d`, where a loader in a bundle looks, and the bundle's
/// Info.plist, of version `version`, as [`info_plist`] says.
fn files(
    package: &str,
    version: &str,
    exe: &Path,
    ggpk: &Path,
    sdk: Option<&Sdk>,
) -> Vec<(String, Content)> {
    let name = |path: &Path| {
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    };
    let (exe_name, ggpk_name) = (name(exe), name(ggpk));
    let copy = |path: &Path| Content::Copy(path.to_owned());
    let Some(sdk) = sdk else {
        return vec![(exe_name, copy(exe)), (ggpk_name, copy(ggpk))];
    };
    let contents = format!("{}/Contents", bundle(package));
    let plist = Content::Text(info_plist(package, version, &exe_name));
    let manifest = Content::Text(sdk.manifest.clone());
    vec![
        (format!("{contents}/Info.plist"), plist),
        (format!("{contents}/MacOS/{exe_name}"), copy(exe)),
        (format!("{contents}/Resources/{ggpk_name}"), copy(ggpk)),
        (
            format!("{contents}/Resources/vulkan/icd.d/{DRIVER_MANIFEST}"),
            manifest,
        ),
        (format!("{contents}/Frameworks/{LOADER}"), copy(&sdk.loader)),
        (format!("{contents}/Frameworks/{DRIVER}"), copy(&sdk.driver)),
    ]
}

/// Game `package`'s app bundle's name: the package's, capitalized, as `Shards.app`.
fn bundle(package: &str) -> String {
    format!("{}.app", capitalized(package))
}

/// `package`, its first letter capitalized.
fn capitalized(package: &str) -> String {
    let mut chars = package.chars();
    let first = chars.next().map(|first| first.to_ascii_uppercase());
    first.into_iter().chain(chars).collect()
}

/// The Info.plist of game `package`'s app bundle, whose executable is `exe`, of version `version`,
/// the package's: its identifier `com.ggengine.<package>`, with `-` for `_`, which identifiers
/// can't hold; its name the package's capitalized, cut to the 15 characters macOS shows; its
/// versions the version's numbers alone, `MAJOR.MINOR.PATCH`, as macOS reads them; for macOS 26 and
/// later, as KosmicKrisp needs; and, as the app starts from Finder or `open`, the loader kept to the
/// bundle's own driver, as Launch Services sets `LSEnvironment` for its launches alone.
fn info_plist(package: &str, version: &str, exe: &str) -> String {
    let identifier = format!("com.ggengine.{}", package.replace('_', "-"));
    let name: String = capitalized(package).chars().take(15).collect();
    let version = version.split(['-', '+']).next().unwrap_or(version);
    let exe = exe
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>{exe}</string>
    <key>CFBundleIdentifier</key>
    <string>{identifier}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>{name}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
    <key>CFBundleVersion</key>
    <string>{version}</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.games</string>
    <key>LSEnvironment</key>
    <dict>
        <key>VK_LOADER_SEARCH_ONLY_IN_BUNDLE</key>
        <string>1</string>
    </dict>
    <key>LSMinimumSystemVersion</key>
    <string>26.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
"#
    )
}

/// What a macOS bundle holds of the Vulkan SDK.
#[derive(Debug, PartialEq)]
struct Sdk {
    /// The loader's file, which the bundle holds as [`LOADER`].
    loader: PathBuf,
    /// KosmicKrisp's library.
    driver: PathBuf,
    /// KosmicKrisp's manifest, naming its library where the bundle holds it, as
    /// [`bundled_manifest`] says.
    manifest: String,
}

/// What a bundle holds of the Vulkan SDK whose `macOS` folder `dir`, `VULKAN_SDK`'s value, names:
/// its loader, the file `lib/libvulkan.1.dylib` links to, which names its release, as
/// `libvulkan.1.4.363.dylib`; and KosmicKrisp, its library in `lib` and its manifest in
/// `share/vulkan/icd.d`.
///
/// Fails with `gg.no_sdk` without `dir`, or if a file isn't there or doesn't read, and with
/// `gg.sdk_version` for a loader older than [`OLDEST_LOADER`], or whose file names no release.
fn sdk(dir: Option<OsString>) -> Result<Sdk, Failure> {
    let install = "install the Vulkan SDK, 1.4.363.0 or later, and set VULKAN_SDK to its `macOS` \
                   folder, whose loader and KosmicKrisp a macOS bundle holds";
    let missing = |what: String| Failure::new("gg.no_sdk", format!("{what}; {install}"));
    let dir = dir.filter(|dir| !dir.is_empty()).map(PathBuf::from);
    let dir = dir.ok_or_else(|| missing("VULKAN_SDK isn't set".to_owned()))?;
    let lib = dir.join("lib");
    let link = lib.join(LOADER);
    let loader = fs::canonicalize(&link)
        .map_err(|error| missing(format!("can't find {}: {error}", link.display())))?;
    let release = (loader.file_name().and_then(|name| name.to_str())).and_then(loader_release);
    if !release.is_some_and(|release| release >= OLDEST_LOADER) {
        let message = format!(
            "the Vulkan SDK's loader is {}, older than 1.4.363, or of no release its name gives; \
             {install}",
            loader.display()
        );
        return Err(Failure::new("gg.sdk_version", message));
    }
    let driver = lib.join(DRIVER);
    if !driver.is_file() {
        return Err(missing(format!("{} isn't there", driver.display())));
    }
    let path = dir.join("share/vulkan/icd.d").join(DRIVER_MANIFEST);
    let shipped = fs::read_to_string(&path)
        .map_err(|error| missing(format!("can't read {}: {error}", path.display())))?;
    let manifest = bundled_manifest(&shipped).ok_or_else(|| {
        let what = "isn't a driver's manifest, naming its library in `ICD.library_path`";
        missing(format!("{} {what}", path.display()))
    })?;
    Ok(Sdk {
        loader,
        driver,
        manifest,
    })
}

/// The release of the loader whose file is named `name`, as `[1, 4, 363]` for
/// `libvulkan.1.4.363.dylib`; `None` for a name that gives none.
fn loader_release(name: &str) -> Option<[u32; 3]> {
    let release = name.strip_prefix("libvulkan.")?.strip_suffix(".dylib")?;
    let mut numbers = release.split('.').map(|number| number.parse::<u32>().ok());
    let release = [numbers.next()??, numbers.next()??, numbers.next()??];
    numbers.next().is_none().then_some(release)
}

/// KosmicKrisp's manifest, `shipped` as the Vulkan SDK ships it, naming its library where a bundle
/// holds it, by its path from the manifest's directory, as the loader resolves it, from
/// `Contents/Resources/vulkan/icd.d` to `Contents/Frameworks`; its other fields as shipped. `None`
/// for text that isn't a driver's manifest, which names its library in `ICD.library_path`.
fn bundled_manifest(shipped: &str) -> Option<String> {
    let mut manifest: Value = serde_json::from_str(shipped).ok()?;
    let library = manifest.get_mut("ICD")?.get_mut("library_path")?;
    if !library.is_string() {
        return None;
    }
    *library = json!(format!("../../../Frameworks/{DRIVER}"));
    let text = serde_json::to_string_pretty(&manifest).ok()?;
    Some(text + "\n")
}

/// The path of `path`, a file's in a shipped folder, `/` between its parts, in directory `dir`.
fn at(dir: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(dir.to_owned(), |at, part| at.join(part))
}

/// Lays `files` out in directory `out`, which is emptied first, and nothing else, and returns each
/// file's path in it, with its size, as gg prints them.
///
/// Fails with `gg.ship` if `out` can't be emptied, or a file can't be written there, naming it.
fn lay_out(out: &Path, files: &[(String, Content)]) -> Result<Vec<Value>, Failure> {
    let failed = |path: &Path, error: io::Error| {
        Failure::new(
            "gg.ship",
            format!("can't lay out {}: {error}", path.display()),
        )
    };
    match fs::remove_dir_all(out) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(failed(out, error)),
        _ => {}
    }
    let mut laid = Vec::new();
    for (path, content) in files {
        let to = at(out, path);
        let wrote = fs::create_dir_all(to.parent().unwrap_or(out)).and_then(|()| match content {
            Content::Copy(from) => fs::copy(from, &to),
            Content::Text(text) => fs::write(&to, text).map(|()| text.len() as u64),
        });
        let bytes = wrote.map_err(|error| failed(&to, error))?;
        laid.push(json!({"path": path, "bytes": bytes}));
    }
    Ok(laid)
}

/// Signs the app bundle `app` ad hoc, inside out, as Apple has bundles signed, never with
/// `--deep`, which it deprecates for signing: each of `libraries`, those it holds, then the bundle,
/// sealing what it holds; without the hardened runtime, under which the app would load no library
/// that a team of its own didn't sign. Then checks the signatures strictly, as Gatekeeper would.
///
/// Fails with `gg.sign` if codesign can't run, or fails, as it said on stderr.
fn sign(app: &Path, libraries: &[PathBuf]) -> Result<(), Failure> {
    for path in libraries.iter().map(PathBuf::as_path).chain([app]) {
        codesign(&["--force", "--sign", "-"], path)?;
    }
    codesign(&["--verify", "--deep", "--strict", "--verbose=2"], app)
}

/// Runs codesign with `args` on `path`, its stdout going to stderr, as gg prints a line of its own.
///
/// Fails with `gg.sign` if it can't run, or fails.
fn codesign(args: &[&str], path: &Path) -> Result<(), Failure> {
    let shown = format!("codesign {} {}", args.join(" "), path.display());
    let failed = |what: String| Failure::new("gg.sign", what);
    let status = (Command::new("codesign").args(args).arg(path))
        .stdout(io::stderr())
        .status()
        .map_err(|error| failed(format!("can't run `{shown}`: {error}")))?;
    if !status.success() {
        return Err(failed(format!("`{shown}` failed, as it said above")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of the test's own, empty.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gg-ship-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a directory");
        dir
    }

    /// KosmicKrisp's manifest as the Vulkan SDK ships it.
    const SHIPPED: &str = r#"{
        "file_format_version": "1.0.1",
        "ICD": {
            "library_path": "../../../lib/libvulkan_kosmickrisp.dylib",
            "api_version": "1.4.335",
            "is_portability_driver": false
        }
    }"#;

    #[test]
    fn a_folder_holds_the_executable_beside_its_package_and_a_bundle_the_sdk_too() {
        let (exe, ggpk) = (Path::new("/t/gg-shards"), Path::new("/t/gg-shards.ggpk"));
        let folder = files("shards", "0.1.0", exe, ggpk, None);
        assert_eq!(
            folder,
            [
                ("gg-shards".to_owned(), Content::Copy(exe.to_owned())),
                ("gg-shards.ggpk".to_owned(), Content::Copy(ggpk.to_owned())),
            ]
        );
        let sdk = Sdk {
            loader: PathBuf::from("/sdk/lib/libvulkan.1.4.363.dylib"),
            driver: PathBuf::from("/sdk/lib/libvulkan_kosmickrisp.dylib"),
            manifest: "{}\n".to_owned(),
        };
        let bundled = files("shards", "0.1.0", exe, ggpk, Some(&sdk));
        let paths: Vec<&str> = bundled.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "Shards.app/Contents/Info.plist",
                "Shards.app/Contents/MacOS/gg-shards",
                "Shards.app/Contents/Resources/gg-shards.ggpk",
                "Shards.app/Contents/Resources/vulkan/icd.d/libkosmickrisp_icd.json",
                "Shards.app/Contents/Frameworks/libvulkan.1.dylib",
                "Shards.app/Contents/Frameworks/libvulkan_kosmickrisp.dylib",
            ]
        );
        let content = |at: usize| &bundled[at].1;
        assert_eq!(content(1), &Content::Copy(exe.to_owned()));
        assert_eq!(content(3), &Content::Text(sdk.manifest.clone()));
        assert_eq!(content(4), &Content::Copy(sdk.loader.clone()));
        assert_eq!(content(5), &Content::Copy(sdk.driver.clone()));
        let Content::Text(plist) = content(0) else {
            panic!("the Info.plist's text");
        };
        assert!(plist.contains("<string>gg-shards</string>"), "{plist}");
    }

    #[test]
    fn the_info_plist_names_the_game_and_its_version_and_keeps_the_loader_in_the_bundle() {
        let plist = info_plist("shards", "0.1.0", "gg-shards");
        let entry = |key: &str, value: &str| format!("<key>{key}</key>\n    {value}\n");
        for (key, value) in [
            ("CFBundleExecutable", "<string>gg-shards</string>"),
            ("CFBundleIdentifier", "<string>com.ggengine.shards</string>"),
            ("CFBundleInfoDictionaryVersion", "<string>6.0</string>"),
            ("CFBundleName", "<string>Shards</string>"),
            ("CFBundlePackageType", "<string>APPL</string>"),
            ("CFBundleShortVersionString", "<string>0.1.0</string>"),
            ("CFBundleVersion", "<string>0.1.0</string>"),
            (
                "LSApplicationCategoryType",
                "<string>public.app-category.games</string>",
            ),
            ("LSMinimumSystemVersion", "<string>26.0</string>"),
            ("NSHighResolutionCapable", "<true/>"),
        ] {
            assert!(plist.contains(&entry(key, value)), "{key} in {plist}");
        }
        let environment = "<key>LSEnvironment</key>\n    <dict>\n        \
                           <key>VK_LOADER_SEARCH_ONLY_IN_BUNDLE</key>\n        \
                           <string>1</string>\n    </dict>\n";
        assert!(plist.contains(environment), "{plist}");
        // A version's numbers alone, an identifier without `_`, a name cut to 15 characters, and
        // an executable's name escaped.
        let plist = info_plist("a_very_long_game_name", "1.2.3-beta.1+7", "gg-a&b");
        for value in [
            "<string>1.2.3</string>",
            "<string>com.ggengine.a-very-long-game-name</string>",
            "<string>A_very_long_gam</string>",
            "<string>gg-a&amp;b</string>",
        ] {
            assert!(plist.contains(value), "{value} in {plist}");
        }
    }

    #[test]
    fn kosmickrisps_manifest_names_its_library_in_the_bundle_and_keeps_the_rest() {
        let bundled = bundled_manifest(SHIPPED).expect("a manifest");
        let bundled: Value = serde_json::from_str(&bundled).expect("JSON");
        let mut want: Value = serde_json::from_str(SHIPPED).expect("JSON");
        want["ICD"]["library_path"] = json!("../../../Frameworks/libvulkan_kosmickrisp.dylib");
        assert_eq!(bundled, want);
        for refused in [
            "{not json",
            r#"{"ICD": {}}"#,
            r#"{"ICD": {"library_path": 7}}"#,
            "[]",
        ] {
            assert_eq!(bundled_manifest(refused), None, "{refused}");
        }
    }

    #[test]
    fn a_loaders_release_is_its_files_name() {
        assert_eq!(loader_release("libvulkan.1.4.363.dylib"), Some([1, 4, 363]));
        assert!(loader_release("libvulkan.1.4.335.dylib") < Some(OLDEST_LOADER));
        assert!(loader_release("libvulkan.1.10.0.dylib") > Some(OLDEST_LOADER));
        for name in [
            "libvulkan.1.dylib",
            "libvulkan.1.4.dylib",
            "libvulkan.1.4.363.1.dylib",
            "libvulkan.1.4.x.dylib",
            "libMoltenVK.dylib",
        ] {
            assert_eq!(loader_release(name), None, "{name}");
        }
    }

    /// Without `VULKAN_SDK`, or with an SDK that has no loader, there's nothing to bundle.
    #[test]
    fn an_sdk_that_isnt_there_is_refused() {
        let empty = scratch("empty-sdk");
        let refused = [None, Some(OsString::new()), Some(empty.clone().into())]
            .map(|dir| sdk(dir).err().map(|failure| failure.code));
        let _ = fs::remove_dir_all(&empty);
        let no_sdk = |code: &Option<String>| code.as_deref() == Some("gg.no_sdk");
        assert!(refused.iter().all(no_sdk), "{refused:?}");
    }

    /// An SDK's loader is the file its link names, which gives its release, refused if older.
    #[cfg(unix)]
    #[test]
    fn an_sdks_loader_is_the_file_its_link_names_of_a_release_new_enough() {
        let fake = |release: &str| {
            let dir = scratch(&format!("sdk-{release}"));
            let (lib, icd) = (dir.join("lib"), dir.join("share/vulkan/icd.d"));
            fs::create_dir_all(&lib).expect("a directory");
            fs::create_dir_all(&icd).expect("a directory");
            let file = format!("libvulkan.{release}.dylib");
            fs::write(lib.join(&file), b"loader").expect("written");
            std::os::unix::fs::symlink(&file, lib.join(LOADER)).expect("linked");
            fs::write(lib.join(DRIVER), b"driver").expect("written");
            fs::write(icd.join(DRIVER_MANIFEST), SHIPPED).expect("written");
            dir
        };
        let (new, old) = (fake("1.4.363"), fake("1.4.341"));
        let found = sdk(Some(new.clone().into()));
        let refused = sdk(Some(old.clone().into()))
            .err()
            .map(|failure| failure.code);
        let canonical = |path: PathBuf| fs::canonicalize(path).expect("there");
        let want = Sdk {
            loader: canonical(new.join("lib/libvulkan.1.4.363.dylib")),
            driver: new.join("lib").join(DRIVER),
            manifest: bundled_manifest(SHIPPED).expect("a manifest"),
        };
        let _ = (fs::remove_dir_all(&new), fs::remove_dir_all(&old));
        assert_eq!(found, Ok(want));
        assert_eq!(refused.as_deref(), Some("gg.sdk_version"));
    }

    /// The folder is emptied of what an earlier ship left, and nothing beside it is touched.
    #[test]
    fn a_folder_is_laid_out_afresh_and_nothing_beside_it_is_touched() {
        let dir = scratch("lay-out");
        let (out, beside) = (dir.join("ship/shards"), dir.join("ship/other/kept"));
        fs::create_dir_all(out.join("Old.app")).expect("a directory");
        fs::write(out.join("Old.app/left"), b"left").expect("written");
        fs::create_dir_all(beside.parent().expect("a parent")).expect("a directory");
        fs::write(&beside, b"kept").expect("written");
        let exe = dir.join("gg-shards");
        fs::write(&exe, b"executable").expect("written");
        let files = [
            (
                "A.app/Contents/MacOS/gg-shards".to_owned(),
                Content::Copy(exe.clone()),
            ),
            (
                "A.app/Contents/Info.plist".to_owned(),
                Content::Text("plist".to_owned()),
            ),
        ];
        let laid = lay_out(&out, &files);
        let read = |path: &str| fs::read(at(&out, path)).ok();
        let (copied, written) = (read(&files[0].0), read(&files[1].0));
        let (left, kept) = (out.join("Old.app").exists(), fs::read(&beside).ok());
        let refused = lay_out(&out, &[("x".to_owned(), Content::Copy(dir.join("gone")))]);
        let _ = fs::remove_dir_all(&dir);
        let want = json!([
            {"path": "A.app/Contents/MacOS/gg-shards", "bytes": 10},
            {"path": "A.app/Contents/Info.plist", "bytes": 5},
        ]);
        assert_eq!(laid.map(Value::from), Ok(want));
        assert_eq!(copied.as_deref(), Some(&b"executable"[..]));
        assert_eq!(written.as_deref(), Some(&b"plist"[..]));
        assert!(!left);
        assert_eq!(kept.as_deref(), Some(&b"kept"[..]));
        let refused = refused.err().map(|failure| failure.code);
        assert_eq!(refused.as_deref(), Some("gg.ship"));
    }
}
