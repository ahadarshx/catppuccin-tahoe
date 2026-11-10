#!/usr/bin/env bash
# Top-bar font + one-line "Title • Artist" for Dynamic Music Pill 1.5.x.
# Install the extension first; re-run after every extension update.
set -euo pipefail
here="$(dirname "$(readlink -f "$0")")"
ext="$HOME/.local/share/gnome-shell/extensions/dynamic-music-pill@andbal"
[ -d "$ext" ] || { echo "Dynamic Music Pill is not installed" >&2; exit 1; }
cd "$ext"
if git apply --check --reverse "$here/patches/dynamic-music-pill.patch" 2>/dev/null; then
    echo "Already patched."
else
    git apply "$here/patches/dynamic-music-pill.patch" && echo "Patched. Log out and back in to load it."
fi
