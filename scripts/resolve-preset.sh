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
else
  # Allow direct preset name for future extensions.
  PRESET_CONFIG="$mode"
  BUILD_PRESET="$mode"
fi
shopt -u nocasematch

export PRESET_CONFIG
export BUILD_PRESET
