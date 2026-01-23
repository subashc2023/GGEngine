# GGEngine

Minimal C++ engine layout with two example apps: `Sandbox` and `Editor`.

## Quick start (Windows)

1. Install these tools:
   - CMake (3.20+)
   - LLVM/Clang
   - Ninja
   - MSYS2 MinGW-w64 (Windows only)

Download pages:

```
https://cmake.org/download/
https://releases.llvm.org/download.html
https://github.com/ninja-build/ninja/releases
https://www.msys2.org/
```

2. Clone the repo with submodules:

```bash
git clone --recurse-submodules <repo-url>
```

If you already cloned without submodules:

```bash
git submodule update --init --recursive
```

3. After installing MSYS2, search for "MSYS2 UCRT64" in your start menu and run:

```powershell
pacman -S mingw-w64-ucrt-x86_64-toolchain
```

4. Build everything:

```powershell
.\scripts\build-all.bat <debug or release>
```

On Linux/macOS:

```bash
./scripts/build-all <debug or release>
```

5. Run:

```powershell
.\bin\Debug-x64\Sandbox\Sandbox.exe
.\bin\Debug-x64\Editor\Editor.exe
```

For release builds, alternate outputs, presets, and tool paths, see `AGENTS.md`.

## Planned Optimizations

Future rendering improvements on the roadmap:
- **Persistent mapped buffers** - Eliminate per-frame vertex buffer uploads
- **Texture atlases** - Reduce texture switches and batch more draw calls
- **GPU-driven rendering** - Dynamic/indirect rendering for large scenes
