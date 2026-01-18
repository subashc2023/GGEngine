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

2. After installing MSYS2, search for "MSYS2 UCRT64" in your start menu and run:

```powershell
pacman -S mingw-w64-ucrt-x86_64-toolchain
```

3. Build everything:

```powershell
.\scripts\build-all.bat <debug or release>
```

On Linux/macOS:

```bash
./scripts/build-all <debug or release>
```

4. Run:

```powershell
.\bin\Debug-x64\Sandbox\Sandbox.exe
.\bin\Debug-x64\Editor\Editor.exe
```

Expected output:

- Sandbox: `Sandbox Add(2, 3) = 5`
- Editor: `Editor Add(10, 20) = 30`

For release builds, alternate outputs, presets, and tool paths, see `AGENTS.md`.
