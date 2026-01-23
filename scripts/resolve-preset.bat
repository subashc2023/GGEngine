@echo off
setlocal

set MODE=%~1
if "%MODE%"=="" set MODE=debug

if /I "%MODE%"=="debug" (
    set PRESET_CONFIG=windows-clang-mingw64
    set BUILD_PRESET=windows-clang-mingw64-debug
) else if /I "%MODE%"=="release" (
    set PRESET_CONFIG=windows-clang-mingw64-release-static
    set BUILD_PRESET=windows-clang-mingw64-release-static
) else if /I "%MODE%"=="dist" (
    set PRESET_CONFIG=windows-clang-mingw64-dist
    set BUILD_PRESET=windows-clang-mingw64-dist
) else (
    rem Allow direct preset name for future extensions.
    set PRESET_CONFIG=%MODE%
    set BUILD_PRESET=%MODE%
)

endlocal & (
    set PRESET_CONFIG=%PRESET_CONFIG%
    set BUILD_PRESET=%BUILD_PRESET%
)
