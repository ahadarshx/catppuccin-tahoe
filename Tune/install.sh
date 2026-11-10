#!/usr/bin/env bash
# Builds Tune and installs it for the current user:
#   ~/.local/bin/tune, its launcher, and its icons (colour app icon for the
#   grid and dock, symbolic one for inside the app) in the hicolor theme,
#   which every icon theme falls back to.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"

cargo build --release
install -Dm755 target/release/tune "$HOME/.local/bin/tune"
# A full path: GNOME's launcher does not always have ~/.local/bin on its PATH.
install -Dm644 data/io.github.rice.Tune.desktop "$HOME/.local/share/applications/io.github.rice.Tune.desktop"
sed -i "s|^Exec=tune$|Exec=$HOME/.local/bin/tune|" "$HOME/.local/share/applications/io.github.rice.Tune.desktop"

icons="$HOME/.local/share/icons/hicolor"
install -Dm644 data/icons/hicolor/scalable/apps/io.github.rice.Tune.svg "$icons/scalable/apps/io.github.rice.Tune.svg"
install -Dm644 data/icons/hicolor/symbolic/apps/io.github.rice.Tune-symbolic.svg "$icons/symbolic/apps/io.github.rice.Tune-symbolic.svg"
# Icons only Tune uses (the heart), found by name in Tune's own folder.
install -Dm644 -t "$HOME/.local/share/tune/icons" data/icons/tune-*-symbolic.svg
# In the rice, Tune shows MacTahoe's Spotify icon; hicolor keeps Tune's own for other themes.
rice_icons="$HOME/.local/share/icons/MacTahoe-Apps"
if [ -d "$rice_icons/apps/scalable" ]; then
    ln -sfn ../../../MacTahoe/apps/scalable/spotify.svg "$rice_icons/apps/scalable/io.github.rice.Tune.svg"
    install -m644 ../icons/rice-spotify-stripes-symbolic.svg "$rice_icons/apps/scalable/"
    ln -sfn rice-spotify-stripes-symbolic.svg "$rice_icons/apps/scalable/io.github.rice.Tune-symbolic.svg"
    gtk-update-icon-cache -f -t "$rice_icons" > /dev/null || true
fi
[ -f "$icons/index.theme" ] && gtk-update-icon-cache -f -t "$icons" > /dev/null || true
update-desktop-database "$HOME/.local/share/applications" 2> /dev/null || true
echo "Installed. Tune is in the app grid; run 'tune' from a terminal for logs."
