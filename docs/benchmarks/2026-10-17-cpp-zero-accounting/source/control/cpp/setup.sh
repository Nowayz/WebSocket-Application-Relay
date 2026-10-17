#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
dep=artifacts/modern-uwebsockets
if [[ ! -d "$dep/.git" ]]; then git clone --no-checkout https://github.com/uNetworking/uWebSockets.git "$dep"; fi
git -C "$dep" checkout --detach 045bbc11ef4f3ec933c112579d3e8922302a798b
git -C "$dep" submodule update --init --depth 1 uSockets
# TLS, QUIC, compression and fuzzing dependencies are intentionally unnecessary.

patch_file="$(pwd)/cpp/patches/war-transport.patch"
if git -C "$dep" apply --reverse --check "$patch_file" 2>/dev/null; then :
else git -C "$dep" apply --check "$patch_file"; git -C "$dep" apply "$patch_file"; fi
