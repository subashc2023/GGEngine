#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# shellcheck source=/dev/null
source "$root/scripts/resolve-preset.sh" "${1:-}"

cmake --preset "$PRESET_CONFIG"
cmake --build --preset "$BUILD_PRESET" --target Editor
