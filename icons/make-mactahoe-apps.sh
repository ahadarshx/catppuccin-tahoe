#!/usr/bin/env bash
# Builds the MacTahoe-Apps icon theme: MacTahoe's app icons only, Adwaita for
# everything else (folders, symbolic icons), plus the rice's custom icons.
# Needs MacTahoe-icon-theme installed in ~/.local/share/icons/MacTahoe.
set -euo pipefail
here="$(dirname "$(readlink -f "$0")")"
icons="$HOME/.local/share/icons"
out="$icons/MacTahoe-Apps"
[ -d "$icons/MacTahoe/apps" ] || { echo "Install MacTahoe-icon-theme first" >&2; exit 1; }
for d in 16 22 32 scalable; do
    mkdir -p "$out/apps/$d"
    for f in "$icons/MacTahoe/apps/$d"/*; do
        [ -d "$f" ] && continue
        case "$f" in *symbolic*.svg) continue ;; esac  # keep Adwaita symbolic icons
        ln -sfn "../../../MacTahoe/apps/$d/$(basename "$f")" "$out/apps/$d/"
    done
done
install -m644 "$here/MacTahoe-Apps/index.theme" "$out/"
sc="$out/apps/scalable"
install -m644 "$here"/rice-*.svg "$sc/"
ln -sfn preferences-color.svg "$sc/art.fatdawlf.Piccolo.svg"
ln -sfn tomboy.svg "$sc/com.toolstack.Folio.svg"
ln -sfn internet-mail.svg "$sc/co.hyprlab.Hylki.svg"
ln -sfn spotify.svg "$sc/io.github.rice.Tune.svg"
ln -sfn rice-spotify-stripes-symbolic.svg "$sc/io.github.rice.Tune-symbolic.svg"
for n in io.github.celluloid_player.Celluloid io.github.celluloid-player.Celluloid io.github.Celluloid; do
    ln -sfn rice-celluloid.svg "$sc/$n.svg"
done
for n in com.stremio.Stremio com.stremio.Service smartcode-stremio stremio; do
    ln -sfn rice-stremio.svg "$sc/$n.svg"
done
gtk-update-icon-cache -f -t "$out" > /dev/null
gsettings set org.gnome.desktop.interface icon-theme MacTahoe-Apps
echo "MacTahoe-Apps installed and selected."
