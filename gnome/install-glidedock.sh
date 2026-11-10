#!/usr/bin/env bash
# Installs GlideDock with the rice tweaks (patched on top of upstream).
set -euo pipefail
here="$(dirname "$(readlink -f "$0")")"
src="${XDG_CACHE_HOME:-$HOME/.cache}/rice/GlideDock"
commit=e7449b0d57db80457456833a2def5780763b628e
if [ ! -d "$src/.git" ]; then
    git clone https://github.com/bquangthien25-hub/GlideDock.git "$src"
fi
cd "$src"
git checkout -q --force "$commit"
git clean -fdq
git apply "$here/patches/glidedock.patch"
./build.sh --install
echo "GlideDock installed. Log out and back in, then enable it in Extension Manager."
