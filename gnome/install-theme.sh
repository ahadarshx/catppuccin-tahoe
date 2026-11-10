#!/usr/bin/env bash
# Clones MacTahoe-gtk-theme at the commit the patch was made against, applies the
# Catppuccin Mocha recolor and installs the dark, solid GTK + GNOME Shell theme.
set -euo pipefail
here="$(dirname "$(readlink -f "$0")")"
src="${XDG_CACHE_HOME:-$HOME/.cache}/rice/MacTahoe-gtk-theme"
commit=1e45e19f510edb8cde18fa84d6cd5319f60b086b
if [ ! -d "$src/.git" ]; then
    git clone https://github.com/vinceliuice/MacTahoe-gtk-theme.git "$src"
fi
cd "$src"
git checkout -q --force "$commit"
git clean -fdq
git apply "$here/patches/mactahoe-catppuccin.patch"
./install.sh -o solid -c dark -l -f -HD --round --shell -i fedora -sf
echo "Theme installed. Select MacTahoe-Dark-solid in GNOME Tweaks / User Themes, or run 'reshell'."
