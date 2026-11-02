#!/usr/bin/env bash
set -euo pipefail

# Keep credentials in the existing literal environment file. This launcher
# selects the reviewed durable mode without changing that file's contents.
void_project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$void_project_dir"
export VOID_MESSAGING_ENABLED=true
export VOID_API_AUDIENCE="${VOID_API_AUDIENCE:-https://api.voidchats.xyz/dm9pZG5vdGhpbmduZXNz}"
exec ./void-server "$@"
