#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

"$root/scripts/clean.sh" "${1:-}"
"$root/scripts/build-all.sh" "${1:-}"
