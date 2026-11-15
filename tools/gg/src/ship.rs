//! `gg ship`: a game built as it ships, without `dev` in the shipping profile, beside the package
//! the cooker of the engine it builds with cooks from its project, laid out in a folder of its own,
//! named as the game is, as gg-api's `Project::name` says, with the notices of what it's made from:
//! on Windows and Linux, whose GPUs' drivers install the Vulkan loader, the executable and its
//! package side by side, on Windows with the Visual C++ runtime; on macOS, an app bundle holding
//! them with the Vulkan SDK's loader and KosmicKrisp, its Vulkan driver over Metal, and the game's
//! icon, signed ad hoc. The build's symbols are laid out in a folder beside it, by the OS's own
//! tools, from which the frames a panic logs are turned into lines, as ARCHITECTURE.md's
//! Diagnostics says.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gg_api::{Failure, Project};
use serde_json::{Value, json};

use crate::{Scope, build, run};

/// What `gg ship` ships: a package, a project's game, and the project gg works in, if any.
pub(crate) struct Ship {
    pub(crate) package: String,
    pub(crate) project: Option<Scope>,
}

/// Cargo's flags for a build as it ships: without the game's `dev`, in the shipping profile, which
/// the workspace's `.cargo/config.toml` gives.
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

/// The notices of the loader and KosmicKrisp, which a bundle's NOTICES.txt adds: their licenses at
/// the Vulkan SDK 1.4.363's release, with the copyright holders their sources name. A bundle of a
/// later SDK's needs them read again.
const BUNDLED_NOTICES: [&str; 2] = [
    include_str!("../notices/vulkan-loader.txt"),
    include_str!("../notices/kosmickrisp.txt"),
];

/// The Visual C++ runtime's DLLs a game's executable imports, for Luau's and VMA's C++, which
/// Windows doesn't include: a folder that holds them beside the executable starts on a PC without
/// the Visual C++ Redistributable, as Microsoft documents for deploying them locally.
const RUNTIME: [&str; 3] = ["vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"];

/// The name of the folder of Visual Studio's redist folder that holds the runtime for the
/// architecture gg runs on, which builds the game.
const RUNTIME_ARCH: &str = if cfg!(target_arch = "aarch64") {
    "arm64"
} else {
    "x64"
};

/// Builds `ship`'s package as it ships, cooks its project into its package beside its executable,
/// where a build without `dev` finds it, with the cooker of the engine it builds with, as
/// [`cooker`] says, and lays the two out in `ship/<package>` in the target
/// directory, emptied first, as [`files`] says, with the notices [`notices`] writes; on Windows, with
/// the Visual C++ runtime, as [`runtime`] finds it; on macOS, with the loader and KosmicKrisp of
/// the Vulkan SDK that `VULKAN_SDK` names and the project's icon, signed as [`sign`] says. Lays the
/// build's symbols out in `ship/<package>-symbols`, as [`symbols`] says. Prints a line of JSON
/// naming each folder and each file in it, with its size.
///
/// Fails as [`run::check_game`] does; with `project.read` or `project.bad` for a package whose
/// directory holds no project that reads, `gg.project` for one whose project names another game,
/// and `gg.cook` for one that doesn't build with gg-engine, whose cooker would cook it; before
/// building, with `gg.notices` if cargo-about can't run, on Windows as [`redist`] and
/// [`runtime`] do, and on macOS as [`sdk`] does, or with `gg.ship` if the icon can't be read;
/// with `gg.build` if the workspace has no such package, it has no binary or it doesn't build,
/// `gg.cook` if its project doesn't cook, and `gg.cargo` if cargo can't run to say what the
/// workspace holds; as [`licenses`] does; with `gg.symbols` if the symbols can't be kept, as
/// [`pdb`], [`strip`] and [`dsym`] say; with `gg.ship` if a folder can't be laid out; and as `sign`
/// does. Folders that fail to be laid out or signed are removed, so that none looks shipped that
/// isn't.
pub(crate) fn ship(ship: &Ship) -> Result<(), Failure> {
    let dir = ship.project.as_ref().map(Scope::dir);
    run::check_game(&ship.package, ship.project.as_ref())?;
    let package = build::package(&ship.package, dir)?;
    let project_dir = package.manifest.parent().unwrap_or(Path::new("."));
    let project = Project::read(project_dir)?;
    if project.game != ship.package {
        let message = format!(
            "the project in {} names its game `{}`, not `{}`, beside whose manifest it is",
            project_dir.display(),
            project.game,
            ship.package
        );
        return Err(Failure::new("gg.project", message));
    }
    let engine = package.engine.as_deref().ok_or_else(|| {
        let message = format!(
            "package `{}` doesn't build with gg-engine, whose cooker cooks a game's project",
            ship.package
        );
        Failure::new("gg.cook", message)
    })?;
    // Before the build, which takes minutes.
    about(&package.root)?;
    let bundle = if cfg!(target_os = "macos") {
        let sdk = sdk(std::env::var_os("VULKAN_SDK"))?;
        let icon = (project.icon_path(project_dir))
            .map(|path| {
                fs::read(&path).map_err(|error| {
                    let message = format!("can't read the icon {}: {error}", path.display());
                    Failure::new("gg.ship", message)
                })
            })
            .transpose()?;
        Some(Bundle { sdk, icon })
    } else {
        None
    };
    let runtime = if cfg!(windows) {
        runtime(&redist(std::env::var_os("VCToolsRedistDir"))?, RUNTIME_ARCH)?
    } else {
        Vec::new()
    };
    let built = build::build(&ship.package, &SHIPPING, dir)?;
    let ggpk = built.exe.with_extension("ggpk");
    let cooker = cooker(engine, &package.root, &package.target);
    cook(project_dir, &ggpk, &cooker, dir)?;

    let name = project.name();
    let credits = project_dir.join("CREDITS.txt");
    let credits = match fs::read_to_string(&credits) {
        Ok(credits) => Some(credits),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            let message = format!("can't read {}: {error}", credits.display());
            return Err(Failure::new("gg.notices", message));
        }
    };
    let written = built
        .exe
        .with_file_name(format!("{}-licenses.txt", package.bin));
    let licenses = licenses(&package.root, &package.manifest, &written)?;
    let bundled = bundle.is_some();
    let notices = notices(
        &name,
        &package.version,
        credits.as_deref(),
        &licenses,
        bundled,
    );
    let kept = if cfg!(windows) {
        Kept::Pdb(pdb(&built.exe)?)
    } else if bundled {
        Kept::Dsym
    } else {
        Kept::Built
    };
    let exe = match kept {
        Kept::Built => strip(&built.exe)?,
        Kept::Pdb(_) | Kept::Dsym => built.exe.clone(),
    };
    let folder = Folder {
        game: &project.game,
        name: &name,
        version: &package.version,
        exe: &exe,
        ggpk: &ggpk,
        notices,
        runtime: &runtime,
        bundle: bundle.as_ref(),
    };
    let files = files(&folder);
    let build_txt = build_text(&project.game, &package.version, built.source.as_deref());
    let symbol_files = symbols(&name, &built.exe, &kept, build_txt);
    let shipped_dir = package.target.join("ship");
    let out = shipped_dir.join(&ship.package);
    let symbols_out = shipped_dir.join(format!("{}-symbols", ship.package));
    let shipped = lay_out(&out, &files).and_then(|laid| {
        if bundled {
            let libraries: Vec<PathBuf> = (files.iter())
                .filter(|(path, _)| path.ends_with(".dylib"))
                .map(|(path, _)| at(&out, path))
                .collect();
            sign(&out.join(format!("{name}.app")), &libraries)?;
        }
        let mut symbols = lay_out(&symbols_out, &symbol_files)?;
        if let Kept::Dsym = kept {
            let at = format!("{name}.dSYM");
            dsym(&built.exe, &symbols_out.join(&at))?;
            symbols.push(json!({"path": at}));
        }
        Ok((laid, symbols))
    });
    if shipped.is_err() {
        let _ = fs::remove_dir_all(&out);
        let _ = fs::remove_dir_all(&symbols_out);
    }
    let (laid, symbols) = shipped?;
    let symbols = json!({"dir": symbols_out, "files": symbols});
    crate::print(
        &json!({"shipped": ship.package, "dir": out, "files": laid, "symbols": symbols})
            .to_string(),
    );
    Ok(())
}

/// Cargo's arguments that run the cooker of the engine whose checkout is `engine`, for a game in
/// the workspace whose root is `root`, built into `target`, up to the cooker's own: the cooker's
/// manifest, in the engine's `tools/cook`; and, for an engine outside the workspace, as a game of
/// its own builds with, a target directory of the cooker's in the game's, `gg-cook`, as the
/// engine's own may be in cargo's checkout of a git revision. In the engine's workspace, the
/// workspace's target, where its other builds are.
fn cooker(engine: &Path, root: &Path, target: &Path) -> Vec<OsString> {
    let manifest = engine.join("tools").join("cook").join("Cargo.toml");
    let mut args: Vec<OsString> = vec!["run".into(), "--manifest-path".into(), manifest.into()];
    if engine != root {
        args.extend(["--target-dir".into(), target.join("gg-cook").into()]);
    }
    args.push("--".into());
    args
}

/// Cooks the project in directory `project` into the package at `ggpk`, with the cooker that
/// `cooker`, cargo's arguments as [`cooker`] gives them, runs, through cargo run in `dir`; the
/// lines the cooker prints go to stderr, as gg prints one of its own.
///
/// Fails with `gg.cook` if cargo can't run, or the cooker fails, as it said on stderr.
fn cook(
    project: &Path,
    ggpk: &Path,
    cooker: &[OsString],
    dir: Option<&Path>,
) -> Result<(), Failure> {
    let failed = |what: String| Failure::new("gg.cook", what);
    let status = (build::cargo(dir).args(cooker))
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

/// What a file of a shipped folder holds: a copy of a file, or bytes written.
#[derive(Debug, PartialEq)]
enum Content {
    Copy(PathBuf),
    Bytes(Vec<u8>),
}

/// What a game's shipped folder is made of.
struct Folder<'a> {
    /// The game's package, and its name, as `Project::name` gives it, which its files take.
    game: &'a str,
    name: &'a str,
    /// Its version, its package's.
    version: &'a str,
    /// The executable players get, and its package.
    exe: &'a Path,
    ggpk: &'a Path,
    /// The text of its NOTICES.txt.
    notices: String,
    /// The Visual C++ runtime's DLLs, on Windows.
    runtime: &'a [PathBuf],
    /// What a macOS bundle holds besides, on macOS.
    bundle: Option<&'a Bundle>,
}

/// What a macOS bundle holds besides the game: the Vulkan SDK's loader and KosmicKrisp, and the
/// project's icon, a PNG's bytes, if it names one.
struct Bundle {
    sdk: Sdk,
    icon: Option<Vec<u8>>,
}

/// The files of `folder`, by their paths in it, `/` between their parts, named after the game, its
/// executable keeping its extension, as `Foxball.exe`: its executable and its package side by side,
/// with the runtime's DLLs; or, with a bundle, as on macOS, an app bundle holding them, named after
/// the game, as Apple lays bundles out, the executable in `Contents/MacOS` and the package, which is
/// data, in `Contents/Resources`, where an executable in a bundle finds it, with the icon as
/// [`icns`] makes it; with the SDK's loader and KosmicKrisp in `Contents/Frameworks`, the manifest
/// by which the loader finds KosmicKrisp in `Contents/Resources/vulkan/icd.d`, where a loader in a
/// bundle looks, and the bundle's Info.plist, as [`info_plist`] says. NOTICES.txt beside them, out
/// of the bundle's seal.
fn files(folder: &Folder) -> Vec<(String, Content)> {
    let name = folder.name;
    let extension = (folder.exe.extension())
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let (exe, ggpk) = (format!("{name}{extension}"), format!("{name}.ggpk"));
    let copy = |path: &Path| Content::Copy(path.to_owned());
    let notices = (
        "NOTICES.txt".to_owned(),
        Content::Bytes(folder.notices.clone().into_bytes()),
    );
    let Some(bundle) = folder.bundle else {
        let mut files = vec![(exe, copy(folder.exe)), (ggpk, copy(folder.ggpk))];
        for dll in folder.runtime {
            let file = dll.file_name().unwrap_or_default().to_string_lossy();
            files.push((file.into_owned(), copy(dll)));
        }
        files.push(notices);
        return files;
    };
    let contents = format!("{name}.app/Contents");
    let icon = bundle.icon.is_some();
    let plist = info_plist(folder.game, name, folder.version, &exe, icon);
    let manifest = Content::Bytes(bundle.sdk.manifest.clone().into_bytes());
    let mut files = vec![
        (
            format!("{contents}/Info.plist"),
            Content::Bytes(plist.into_bytes()),
        ),
        (format!("{contents}/MacOS/{exe}"), copy(folder.exe)),
        (format!("{contents}/Resources/{ggpk}"), copy(folder.ggpk)),
        (
            format!("{contents}/Resources/vulkan/icd.d/{DRIVER_MANIFEST}"),
            manifest,
        ),
        (
            format!("{contents}/Frameworks/{LOADER}"),
            copy(&bundle.sdk.loader),
        ),
        (
            format!("{contents}/Frameworks/{DRIVER}"),
            copy(&bundle.sdk.driver),
        ),
    ];
    if let Some(png) = &bundle.icon {
        let icns = Content::Bytes(icns(png));
        files.push((format!("{contents}/Resources/{name}.icns"), icns));
    }
    files.push(notices);
    files
}

/// An icns holding `png`, a PNG of 256 by 256 pixels, as its one entry, `ic08`, which macOS reads
/// as it is: the file's type and length, then the entry's, each length its header's 8 bytes and
/// what follows, big-endian.
fn icns(png: &[u8]) -> Vec<u8> {
    let entry = 8 + png.len() as u32;
    let mut icns = b"icns".to_vec();
    icns.extend((8 + entry).to_be_bytes());
    icns.extend(b"ic08");
    icns.extend(entry.to_be_bytes());
    icns.extend(png);
    icns
}

/// The Info.plist of the app bundle of game `game`, named `name`, whose executable is `exe`, of
/// version `version`, the package's: its identifier `com.ggengine.<game>`, with `-` for `_`, which
/// identifiers can't hold; its name cut to the 15 characters macOS shows; its versions the
/// version's numbers alone, `MAJOR.MINOR.PATCH`, as macOS reads them; with `icon`, its icon
/// `<name>.icns`; for macOS 26 and later, as KosmicKrisp needs; and, as the app starts from Finder or
/// `open`, the loader kept to the bundle's own driver, as Launch Services sets `LSEnvironment` for
/// its launches alone.
fn info_plist(game: &str, name: &str, version: &str, exe: &str, icon: bool) -> String {
    let identifier = format!("com.ggengine.{}", game.replace('_', "-"));
    let escaped = |text: &str| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let short: String = name.chars().take(15).collect();
    let version = version.split(['-', '+']).next().unwrap_or(version);
    let exe = escaped(exe);
    let icon = if icon {
        format!(
            "\n    <key>CFBundleIconFile</key>\n    <string>{}.icns</string>",
            escaped(name)
        )
    } else {
        String::new()
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>{exe}</string>{icon}
    <key>CFBundleIdentifier</key>
    <string>{identifier}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>{short}</string>
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

/// The redist folder of the Visual Studio whose C++ tools build the game, which holds the Visual C++
/// runtime: `var`, `VCToolsRedistDir`, as a developer prompt sets it; or else the newest Visual
/// Studio's, previews included, as vswhere, which Visual Studio's installer installs, finds it,
/// at the version its `VC/Auxiliary/Build/Microsoft.VCRedistVersion.default.txt` names. A newer
/// runtime runs what an older Visual Studio built, as Microsoft keeps it compatible.
///
/// Fails with `gg.no_redist` if vswhere can't run or finds no Visual Studio, or the version can't be
/// read.
fn redist(var: Option<OsString>) -> Result<PathBuf, Failure> {
    if let Some(dir) = var.filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let failed = |what: String| {
        let message = format!(
            "{what}; a game shipped for Windows holds the Visual C++ runtime, which Visual Studio's \
             C++ tools install"
        );
        Failure::new("gg.no_redist", message)
    };
    let programs = std::env::var_os("ProgramFiles(x86)").unwrap_or_default();
    let vswhere = Path::new(&programs).join("Microsoft Visual Studio/Installer/vswhere.exe");
    let found = (Command::new(&vswhere))
        .args(["-latest", "-prerelease", "-products", "*"])
        .args(["-property", "installationPath"])
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| failed(format!("can't run {}: {error}", vswhere.display())))?;
    let install = String::from_utf8_lossy(&found.stdout).trim().to_owned();
    if !found.status.success() || install.is_empty() {
        return Err(failed(format!(
            "{} found no Visual Studio",
            vswhere.display()
        )));
    }
    let install = PathBuf::from(install);
    let default = install.join("VC/Auxiliary/Build/Microsoft.VCRedistVersion.default.txt");
    let version = fs::read_to_string(&default)
        .map_err(|error| failed(format!("can't read {}: {error}", default.display())))?;
    Ok(install.join("VC/Redist/MSVC").join(version.trim()))
}

/// The Visual C++ runtime's DLLs, as [`RUNTIME`] names them, in redist folder `redist`, for
/// architecture `arch`, as Visual Studio names its folders: in its `<arch>/Microsoft.VC*.CRT`.
///
/// Fails with `gg.no_redist` if that folder or a DLL isn't there.
fn runtime(redist: &Path, arch: &str) -> Result<Vec<PathBuf>, Failure> {
    let failed = |what: String| {
        let message = format!("{what}, the Visual C++ runtime a game shipped for Windows holds");
        Failure::new("gg.no_redist", message)
    };
    let arch_dir = redist.join(arch);
    let crt = fs::read_dir(&arch_dir)
        .map_err(|error| failed(format!("can't read {}: {error}", arch_dir.display())))?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .find(|path| {
            (path.file_name().and_then(|name| name.to_str()))
                .is_some_and(|name| name.starts_with("Microsoft.VC") && name.ends_with(".CRT"))
        })
        .ok_or_else(|| failed(format!("{} holds no Microsoft.VC*.CRT", arch_dir.display())))?;
    let dlls = RUNTIME.map(|dll| crt.join(dll));
    if let Some(missing) = dlls.iter().find(|dll| !dll.is_file()) {
        return Err(failed(format!("{} isn't there", missing.display())));
    }
    Ok(dlls.into())
}

/// Checks that cargo-about runs, through cargo run in the workspace's root, `root`.
///
/// Fails with `gg.notices` if it doesn't, saying how to install it.
fn about(root: &Path) -> Result<(), Failure> {
    let ran = (build::cargo(Some(root)).args(["about", "--version"]))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if ran.is_ok_and(|status| status.success()) {
        return Ok(());
    }
    let message = "cargo-about, which writes a shipped game's notices, doesn't run: install it, \
                   `cargo install --locked --features cli cargo-about@0.9.2`";
    Err(Failure::new("gg.notices", message))
}

/// The licenses of the game whose manifest is `manifest`, in the workspace at `root`, as its build
/// as it ships depends on its crates, on the OS and architecture gg runs on: cargo-about's, with the
/// workspace's `about.toml` and gg's `notices.hbs`, offline and with `Cargo.lock` as it is, which
/// a build made before has fetched what it names, written at `out`, beside the build, and read back;
/// what cargo-about logs going to stderr.
///
/// Fails with `gg.notices` if rustc can't say what it builds for, or cargo-about fails, as for a
/// license `about.toml` doesn't accept, as it said on stderr, or what it wrote can't be read.
fn licenses(root: &Path, manifest: &Path, out: &Path) -> Result<String, Failure> {
    let failed = |what: String| Failure::new("gg.notices", what);
    let rustc = (Command::new("rustc").arg("-vV").current_dir(root))
        .output()
        .map_err(|error| failed(format!("can't run rustc: {error}")))?;
    let version = String::from_utf8_lossy(&rustc.stdout);
    let host = (version.lines()).find_map(|line| line.strip_prefix("host: "));
    let host = host.ok_or_else(|| failed("`rustc -vV` named no host".to_owned()))?;
    let template = Path::new(env!("CARGO_MANIFEST_DIR")).join("notices.hbs");
    // Into a file, as cargo-about refuses to write to a pipe where it finds PowerShell, which
    // would re-encode it.
    let status = (build::cargo(Some(root)).args(["about", "generate", "--fail", "--frozen"]))
        .args(["--no-default-features", "--target", host])
        .arg("--config")
        .arg(root.join("about.toml"))
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--output-file")
        .arg(out)
        .arg(&template)
        .stdout(io::stderr())
        .status()
        .map_err(|error| failed(format!("can't run cargo-about: {error}")))?;
    if !status.success() {
        let message = format!(
            "cargo-about couldn't write the notices of {}, as it said above; a license the \
             workspace's about.toml doesn't accept is added there once someone has read it",
            manifest.display()
        );
        return Err(failed(message));
    }
    fs::read_to_string(out)
        .map_err(|error| failed(format!("can't read {}: {error}", out.display())))
}

/// The text of a shipped folder's NOTICES.txt, for game `name` of version `version`: its name and
/// version; its project's `credits`, `CREDITS.txt` beside its manifest, as they are, if it has
/// them; the `licenses` of the crates it's built from, as [`licenses`] gives them; and with
/// `bundled`, those of the loader and KosmicKrisp a macOS bundle holds.
fn notices(
    name: &str,
    version: &str,
    credits: Option<&str>,
    licenses: &str,
    bundled: bool,
) -> String {
    const RULE: &str =
        "================================================================================";
    let mut text = format!("{name} {version}\n");
    let bundled = bundled.then_some(BUNDLED_NOTICES).into_iter().flatten();
    for part in credits.into_iter().chain([licenses]) {
        text.push('\n');
        text.push_str(part.trim_end());
        text.push('\n');
    }
    for part in bundled {
        text.push_str(&format!("\n{RULE}\n{}\n", part.trim_end()));
    }
    text
}

/// How a build's debug info is kept apart from what players get, by the OS's own tools.
enum Kept {
    /// Windows': the PDB the linker wrote beside the executable, which the executable names by its
    /// file name, as rustc has it.
    Pdb(PathBuf),
    /// Linux's: the executable as built, the players' copy stripped of its debug info, as
    /// [`strip`] says.
    Built,
    /// macOS's: a dSYM, as [`dsym`] makes it.
    Dsym,
}

/// The files of the symbols folder of game `name`, built as `exe`, its debug info kept as `kept`
/// says, and `build`, `build.txt`, as [`build_text`] writes it: on Windows, the executable, named
/// as the players' is, and its PDB; on Linux, the executable as built, named so; on macOS nothing
/// more, as the dSYM is made in the folder once it's laid out.
fn symbols(name: &str, exe: &Path, kept: &Kept, build: String) -> Vec<(String, Content)> {
    let mut files = Vec::new();
    let extension = (exe.extension())
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    match kept {
        Kept::Pdb(pdb) => {
            let file = pdb.file_name().unwrap_or_default().to_string_lossy();
            files.push((format!("{name}{extension}"), Content::Copy(exe.to_owned())));
            files.push((file.into_owned(), Content::Copy(pdb.clone())));
        }
        Kept::Built => files.push((name.to_owned(), Content::Copy(exe.to_owned()))),
        Kept::Dsym => {}
    }
    files.push(("build.txt".to_owned(), Content::Bytes(build.into_bytes())));
    files
}

/// `build.txt`, which says which build a symbols folder is of, as a log's `Starting GGEngine` line
/// does: game `game`, its version, `version`, the digest of the source its code is compiled from,
/// `source`, if its build gave one, and the OS and architecture it's built for, gg's own.
fn build_text(game: &str, version: &str, source: Option<&str>) -> String {
    let mut text = format!("game {game}\nversion {version}\n");
    if let Some(source) = source {
        text.push_str(&format!("source {source}\n"));
    }
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    text.push_str(&format!("os {os}\narch {arch}\n"));
    text
}

/// The PDB of the executable `exe`, beside it as rustc names it, after its crate, with `_` for `-`.
///
/// Fails with `gg.symbols` if it isn't there.
fn pdb(exe: &Path) -> Result<PathBuf, Failure> {
    let stem = exe.file_stem().unwrap_or_default().to_string_lossy();
    let pdb = exe.with_file_name(format!("{}.pdb", stem.replace('-', "_")));
    if !pdb.is_file() {
        let message = format!("the build left no PDB, {}", pdb.display());
        return Err(Failure::new("gg.symbols", message));
    }
    Ok(pdb)
}

/// A copy of the executable `exe` stripped of its debug info, by binutils' objcopy, beside it, as
/// `gg-foxball-stripped`, which players get: its symbols stay, which name its functions in a
/// panic's backtrace.
///
/// Fails with `gg.symbols` if objcopy can't run or fails.
fn strip(exe: &Path) -> Result<PathBuf, Failure> {
    let name = exe.file_name().unwrap_or_default().to_string_lossy();
    let stripped = exe.with_file_name(format!("{name}-stripped"));
    let mut objcopy = Command::new("objcopy");
    objcopy.arg("--strip-debug").args([exe, &stripped]);
    tool(objcopy, "objcopy")?;
    Ok(stripped)
}

/// A dSYM of the executable `exe`, made by dsymutil from the objects its debug map names, at `out`.
///
/// Fails with `gg.symbols` if dsymutil can't run or fails.
fn dsym(exe: &Path, out: &Path) -> Result<(), Failure> {
    let mut dsymutil = Command::new("dsymutil");
    dsymutil.arg(exe).arg("-o").arg(out);
    tool(dsymutil, "dsymutil")
}

/// Runs `command`, a tool `name`, its stdout going to stderr, as gg prints a line of its own.
///
/// Fails with `gg.symbols` if it can't run, or fails.
fn tool(mut command: Command, name: &str) -> Result<(), Failure> {
    let failed = |what: String| Failure::new("gg.symbols", what);
    let status = (command.stdout(io::stderr()).status())
        .map_err(|error| failed(format!("can't run {name}: {error}")))?;
    if !status.success() {
        return Err(failed(format!("{name} failed, as it said above")));
    }
    Ok(())
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
            Content::Bytes(bytes) => fs::write(&to, bytes).map(|()| bytes.len() as u64),
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

    /// A folder of Shards, its executable `exe`, its runtime `runtime` and its bundle `bundle`.
    fn shards<'a>(exe: &'a Path, runtime: &'a [PathBuf], bundle: Option<&'a Bundle>) -> Folder<'a> {
        Folder {
            game: "shards",
            name: "Shards",
            version: "0.1.0",
            exe,
            ggpk: Path::new("/t/gg-shards.ggpk"),
            notices: "Shards 0.1.0\n".to_owned(),
            runtime,
            bundle,
        }
    }

    #[test]
    fn a_folder_holds_the_game_by_its_name_beside_its_package_and_notices() {
        let ggpk = Content::Copy(PathBuf::from("/t/gg-shards.ggpk"));
        let notices = Content::Bytes(b"Shards 0.1.0\n".to_vec());
        // On Linux, the stripped copy.
        let stripped = Path::new("/t/gg-shards-stripped");
        let folder = files(&shards(stripped, &[], None));
        assert_eq!(
            folder,
            [
                ("Shards".to_owned(), Content::Copy(stripped.to_owned())),
                ("Shards.ggpk".to_owned(), ggpk),
                ("NOTICES.txt".to_owned(), notices),
            ]
        );
        // On Windows, the runtime beside it.
        let exe = Path::new("/t/gg-shards.exe");
        let runtime = RUNTIME.map(|dll| Path::new("/crt").join(dll));
        let folder = files(&shards(exe, &runtime, None));
        let paths: Vec<&str> = folder.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "Shards.exe",
                "Shards.ggpk",
                "vcruntime140.dll",
                "vcruntime140_1.dll",
                "msvcp140.dll",
                "NOTICES.txt",
            ]
        );
        assert_eq!(folder[0].1, Content::Copy(exe.to_owned()));
        assert_eq!(folder[3].1, Content::Copy(runtime[1].clone()));
    }

    #[test]
    fn a_bundle_holds_the_game_its_icon_and_the_sdk_with_its_notices_beside_it() {
        let sdk = Sdk {
            loader: PathBuf::from("/sdk/lib/libvulkan.1.4.363.dylib"),
            driver: PathBuf::from("/sdk/lib/libvulkan_kosmickrisp.dylib"),
            manifest: "{}\n".to_owned(),
        };
        let bundle = Bundle {
            sdk,
            icon: Some(b"\x89PNG".to_vec()),
        };
        let exe = Path::new("/t/gg-shards");
        let bundled = files(&shards(exe, &[], Some(&bundle)));
        let paths: Vec<&str> = bundled.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "Shards.app/Contents/Info.plist",
                "Shards.app/Contents/MacOS/Shards",
                "Shards.app/Contents/Resources/Shards.ggpk",
                "Shards.app/Contents/Resources/vulkan/icd.d/libkosmickrisp_icd.json",
                "Shards.app/Contents/Frameworks/libvulkan.1.dylib",
                "Shards.app/Contents/Frameworks/libvulkan_kosmickrisp.dylib",
                "Shards.app/Contents/Resources/Shards.icns",
                "NOTICES.txt",
            ]
        );
        let content = |at: usize| &bundled[at].1;
        assert_eq!(content(1), &Content::Copy(exe.to_owned()));
        assert_eq!(content(3), &Content::Bytes(b"{}\n".to_vec()));
        assert_eq!(content(4), &Content::Copy(bundle.sdk.loader.clone()));
        assert_eq!(content(5), &Content::Copy(bundle.sdk.driver.clone()));
        assert_eq!(content(6), &Content::Bytes(icns(b"\x89PNG")));
        let Content::Bytes(plist) = content(0) else {
            panic!("the Info.plist's text");
        };
        let plist = String::from_utf8_lossy(plist);
        assert!(plist.contains("<string>Shards</string>"), "{plist}");
        assert!(plist.contains("<string>Shards.icns</string>"), "{plist}");
        // Without an icon, there's none, and the Info.plist names none.
        let bundle = Bundle {
            icon: None,
            ..bundle
        };
        let bundled = files(&shards(exe, &[], Some(&bundle)));
        assert!(!bundled.iter().any(|(path, _)| path.ends_with(".icns")));
        let Content::Bytes(plist) = &bundled[0].1 else {
            panic!("the Info.plist's text");
        };
        assert!(!String::from_utf8_lossy(plist).contains("CFBundleIconFile"));
    }

    #[test]
    fn an_icns_holds_the_png_as_its_256_pixel_entry() {
        let png = b"\x89PNG\r\n\x1a\n and the rest";
        let icns = icns(png);
        let mut want = b"icns".to_vec();
        want.extend((16 + png.len() as u32).to_be_bytes());
        want.extend(b"ic08");
        want.extend((8 + png.len() as u32).to_be_bytes());
        want.extend(png);
        assert_eq!(icns, want);
    }

    #[test]
    fn the_info_plist_names_the_game_and_its_version_and_keeps_the_loader_in_the_bundle() {
        let plist = info_plist("shards", "Shards", "0.1.0", "Shards", true);
        let entry = |key: &str, value: &str| format!("<key>{key}</key>\n    {value}\n");
        for (key, value) in [
            ("CFBundleExecutable", "<string>Shards</string>"),
            ("CFBundleIconFile", "<string>Shards.icns</string>"),
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
        let plist = info_plist(
            "a_very_long_game_name",
            "A_very_long_game_name",
            "1.2.3-beta.1+7",
            "A&b",
            false,
        );
        for value in [
            "<string>1.2.3</string>",
            "<string>com.ggengine.a-very-long-game-name</string>",
            "<string>A_very_long_gam</string>",
            "<string>A&amp;b</string>",
        ] {
            assert!(plist.contains(value), "{value} in {plist}");
        }
        assert!(!plist.contains("CFBundleIconFile"), "{plist}");
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

    /// The runtime is the three DLLs in the architecture's CRT folder, not onecore's, which
    /// targets other editions of Windows.
    #[test]
    fn the_runtime_is_found_in_the_architectures_crt_folder() {
        let redist = scratch("redist");
        for dir in ["x64/Microsoft.VC145.CRT", "onecore/x64/Microsoft.VC145.CRT"] {
            fs::create_dir_all(redist.join(dir)).expect("a directory");
            for dll in RUNTIME {
                fs::write(redist.join(dir).join(dll), b"dll").expect("written");
            }
        }
        fs::write(redist.join("x64/vc_redist.x64.exe"), b"installer").expect("written");
        let found = runtime(&redist, "x64");
        let want: Vec<PathBuf> = RUNTIME
            .iter()
            .map(|dll| redist.join("x64/Microsoft.VC145.CRT").join(dll))
            .collect();
        let arm = runtime(&redist, "arm64").err().map(|failure| failure.code);
        fs::remove_file(redist.join("x64/Microsoft.VC145.CRT/vcruntime140_1.dll"))
            .expect("removed");
        let short = runtime(&redist, "x64").err().map(|failure| failure.code);
        let _ = fs::remove_dir_all(&redist);
        assert_eq!(found, Ok(want));
        assert_eq!(arm.as_deref(), Some("gg.no_redist"));
        assert_eq!(short.as_deref(), Some("gg.no_redist"));
    }

    #[test]
    fn the_notices_name_the_game_then_its_credits_then_the_licenses() {
        let notices_of =
            |credits, bundled| notices("Shards", "0.1.0", credits, "Licenses.\n\n", bundled);
        assert_eq!(
            notices_of(Some("Credits.\n"), false),
            "Shards 0.1.0\n\nCredits.\n\nLicenses.\n"
        );
        assert_eq!(notices_of(None, false), "Shards 0.1.0\n\nLicenses.\n");
        let bundled = notices_of(None, true);
        assert!(
            bundled.starts_with("Shards 0.1.0\n\nLicenses.\n\n====="),
            "{bundled}"
        );
        for part in BUNDLED_NOTICES {
            assert!(bundled.contains(part.trim_end()), "{part}");
        }
        assert!(BUNDLED_NOTICES[0].contains("Apache License"));
        assert!(BUNDLED_NOTICES[1].contains("MIT License"));
    }

    #[test]
    fn the_symbols_are_the_os_s_own_with_the_build_they_are_of() {
        let pdb = PathBuf::from("/t/gg_shards.pdb");
        let build = || "game shards\n".to_owned();
        let paths = |files: Vec<(String, Content)>| -> Vec<String> {
            files.into_iter().map(|(path, _)| path).collect()
        };
        let windows = symbols(
            "Shards",
            Path::new("/t/gg-shards.exe"),
            &Kept::Pdb(pdb.clone()),
            build(),
        );
        assert_eq!(windows[1], ("gg_shards.pdb".to_owned(), Content::Copy(pdb)));
        assert_eq!(paths(windows), ["Shards.exe", "gg_shards.pdb", "build.txt"]);
        let linux = symbols("Shards", Path::new("/t/gg-shards"), &Kept::Built, build());
        assert_eq!(linux[0].1, Content::Copy(PathBuf::from("/t/gg-shards")));
        assert_eq!(paths(linux), ["Shards", "build.txt"]);
        let macos = symbols("Shards", Path::new("/t/gg-shards"), &Kept::Dsym, build());
        assert_eq!(
            macos,
            [("build.txt".to_owned(), Content::Bytes(build().into_bytes()))]
        );
        let text = build_text("shards", "0.1.0", Some("5c7cd8abeeed5f44"));
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        let want =
            format!("game shards\nversion 0.1.0\nsource 5c7cd8abeeed5f44\nos {os}\narch {arch}\n");
        assert_eq!(text, want);
        assert!(!build_text("shards", "0.1.0", None).contains("source"));
    }

    #[test]
    fn a_pdb_is_named_after_the_crate_and_its_absence_is_refused() {
        let dir = scratch("pdb");
        let exe = dir.join("gg-shards.exe");
        let missing = pdb(&exe).err().map(|failure| failure.code);
        fs::write(dir.join("gg_shards.pdb"), b"pdb").expect("written");
        let found = pdb(&exe);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(missing.as_deref(), Some("gg.symbols"));
        assert_eq!(found, Ok(dir.join("gg_shards.pdb")));
    }

    /// The cooker is the engine's, built where the workspace's builds are in the engine's workspace,
    /// and apart in the game's target for an engine outside it.
    #[test]
    fn the_cooker_is_the_engines_built_apart_for_an_engine_outside() {
        let args = |engine: &str, root: &str| -> Vec<PathBuf> {
            let target = Path::new(root).join("target");
            let args = cooker(Path::new(engine), Path::new(root), &target);
            args.into_iter().map(PathBuf::from).collect()
        };
        let paths = |args: &[&str]| -> Vec<PathBuf> { args.iter().map(PathBuf::from).collect() };
        let inside = ["run", "--manifest-path", "/w/tools/cook/Cargo.toml", "--"];
        assert_eq!(args("/w", "/w"), paths(&inside));
        let outside = [
            "run",
            "--manifest-path",
            "/e/tools/cook/Cargo.toml",
            "--target-dir",
            "/g/target/gg-cook",
            "--",
        ];
        assert_eq!(args("/e", "/g"), paths(&outside));
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
                Content::Bytes(b"plist".to_vec()),
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
