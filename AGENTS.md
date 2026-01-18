# GGEngine developer notes

This file holds dev-specific details to keep `README.md` beginner friendly.

Override the MinGW root before configuring:

```powershell
$env:MINGW_ROOT="D:\msys64\mingw64"
```

## Build presets

Debug (shared DLL):

```powershell
cmake --preset windows-clang-mingw64
cmake --build --preset windows-clang-mingw64-debug
```

Release (static Engine, no DLL):

```powershell
cmake --preset windows-clang-mingw64-release-static
cmake --build --preset windows-clang-mingw64-release-static
```

Linux/macOS (Clang + Ninja) use the same debug/release mode; presets are:

- `linux-clang-ninja` / `linux-clang-ninja-release-static`
- `macos-clang-ninja` / `macos-clang-ninja-release-static`

## Batch scripts

All build scripts accept an optional mode:

```powershell
.\scripts\build-all.bat
.\scripts\build-all.bat debug
.\scripts\build-all.bat release
```

Advanced: pass a preset name directly (configure + build preset):

```powershell
.\scripts\build-all.bat windows-clang-mingw64-release-static
```

Target-specific:

```powershell
.\scripts\build-engine.bat
.\scripts\build-sandbox.bat
.\scripts\build-editor.bat
```

## Bash scripts (Linux/macOS)

Each batch script has a bash equivalent and a cross-platform wrapper:

```bash
./scripts/build-all
./scripts/build-all debug
./scripts/build-all release
```

Direct bash variants:

```bash
./scripts/build-all.sh
./scripts/build-engine.sh
./scripts/build-sandbox.sh
./scripts/build-editor.sh
```

## Output locations

Debug outputs:

```
bin\Debug-x64\Engine\GGEngine.dll
bin\Debug-x64\Sandbox\Sandbox.exe
bin\Debug-x64\Editor\Editor.exe
```

Release static outputs:

```
bin\Release-x64\Engine\GGEngine.a
bin\Release-x64\Sandbox\Sandbox.exe
bin\Release-x64\Editor\Editor.exe
```

## VS Code (optional)

Preconfigured tasks and launch configs live in `.vscode\`:
- Tasks call the batch scripts in `scripts\`.
- Launch configs use GDB at `C:\msys64\mingw64\bin\gdb.exe`.

If your GDB path differs, update `.vscode\launch.json`.

## Clean

```powershell
.\scripts\clean.bat
.\scripts\clean_and_build.bat
```
