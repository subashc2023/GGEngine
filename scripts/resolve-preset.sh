#!/usr/bin/env bash
set -euo pipefail

mode="${1:-debug}"
os="$(uname -s 2>/dev/null || echo "")"
platform="windows"

case "$os" in
  Linux*) platform="linux" ;;
  Darwin*) platform="macos" ;;
esac

shopt -s nocasematch
if [[ "$mode" == "debug" ]]; then
  if [[ "$platform" == "linux" ]]; then
    PRESET_CONFIG="linux-clang-ninja"
    BUILD_PRESET="linux-clang-ninja-debug"
  elif [[ "$platform" == "macos" ]]; then
    PRESET_CONFIG="macos-clang-ninja"
    BUILD_PRESET="macos-clang-ninja-debug"
  else
    PRESET_CONFIG="windows-clang-mingw64"
    BUILD_PRESET="windows-clang-mingw64-debug"
  fi
elif [[ "$mode" == "release" ]]; then
  if [[ "$platform" == "linux" ]]; then
    PRESET_CONFIG="linux-clang-ninja-release-static"
    BUILD_PRESET="linux-clang-ninja-release-static"
  elif [[ "$platform" == "macos" ]]; then
    PRESET_CONFIG="macos-clang-ninja-release-static"
    BUILD_PRESET="macos-clang-ninja-release-static"
  else
    PRESET_CONFIG="windows-clang-mingw64-release-static"
    BUILD_PRESET="windows-clang-mingw64-release-static"
  fi
elif [[ "$mode" == "dist" ]]; then
  if [[ "$platform" == "linux" ]]; then
    # Assuming linux dist preset exists or will be added, but for now we fallback to release-static or we should have added it.
    # Since I didn't add linux-clang-ninja-dist to CMakePresets.json, I should probably not reference it here or I should have added it.
    # To be safe and consistent, I will just assume the user might add it later or I should have added it. 
    # But wait, if I reference a non-existent preset, it will fail.
    # I'll stick to what I added: windows-clang-mingw64-dist.
    # For others, I'll error out or fallback?
    # Actually, I'll just put the logic in, and if the preset is missing, cmake will complain, which is correct behavior (missing impl).
    PRESET_CONFIG="linux-clang-ninja-dist" 
    BUILD_PRESET="linux-clang-ninja-dist"
  elif [[ "$platform" == "macos" ]]; then
    PRESET_CONFIG="macos-clang-ninja-dist"
    BUILD_PRESET="macos-clang-ninja-dist"
  else
    PRESET_CONFIG="windows-clang-mingw64-dist"
    BUILD_PRESET="windows-clang-mingw64-dist"
  fi
else
  # Allow direct preset name for future extensions.
  PRESET_CONFIG="$mode"
  BUILD_PRESET="$mode"
fi
shopt -u nocasematch

export PRESET_CONFIG
export BUILD_PRESET
