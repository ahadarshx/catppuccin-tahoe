<div align="center">

# Catppuccin Tahoe

**A macOS Tahoe-style GNOME desktop in Catppuccin Mocha.**<br>
Recoloured MacTahoe, a glide dock, a music pill and a matching terminal.

<img alt="OS: Fedora 45" src="https://img.shields.io/badge/OS-Fedora%2045-89b4fa?style=flat-square&labelColor=313244&logo=fedora&logoColor=cdd6f4"> <img alt="DE: GNOME 51" src="https://img.shields.io/badge/DE-GNOME%2051-89b4fa?style=flat-square&labelColor=313244&logo=gnome&logoColor=cdd6f4"> <img alt="Theme: MacTahoe × Catppuccin Mocha" src="https://img.shields.io/badge/Theme-MacTahoe%20%C3%97%20Catppuccin%20Mocha-cba6f7?style=flat-square&labelColor=313244"> <img alt="Icons: MacTahoe + Adwaita" src="https://img.shields.io/badge/Icons-MacTahoe%20%2B%20Adwaita-cba6f7?style=flat-square&labelColor=313244"> <img alt="Cursor: MacTahoe" src="https://img.shields.io/badge/Cursor-MacTahoe-cba6f7?style=flat-square&labelColor=313244"><br>
<img alt="Dock: GlideDock" src="https://img.shields.io/badge/Dock-GlideDock-f5c2e7?style=flat-square&labelColor=313244"> <img alt="Top bar: Kiwi · Music Pill · Vitals" src="https://img.shields.io/badge/Top%20bar-Kiwi%20%C2%B7%20Music%20Pill%20%C2%B7%20Vitals-f5c2e7?style=flat-square&labelColor=313244"> <img alt="Launcher: Vicinae" src="https://img.shields.io/badge/Launcher-Vicinae-f5c2e7?style=flat-square&labelColor=313244"><br>
<img alt="Terminal: Ghostty + zsh" src="https://img.shields.io/badge/Terminal-Ghostty%20%2B%20zsh-a6e3a1?style=flat-square&labelColor=313244&logo=ghostty&logoColor=cdd6f4"> <img alt="Font: Adwaita Sans · Cascadia Code NF" src="https://img.shields.io/badge/Font-Adwaita%20Sans%20%C2%B7%20Cascadia%20Code%20NF-a6e3a1?style=flat-square&labelColor=313244"> <img alt="Music: Tune (GTK4 Spotify)" src="https://img.shields.io/badge/Music-Tune%20%28GTK4%20Spotify%29-a6e3a1?style=flat-square&labelColor=313244&logo=spotify&logoColor=cdd6f4"><br>
<img alt="Themed: Zen · VS Code · OBS" src="https://img.shields.io/badge/Themed-Zen%20%C2%B7%20VS%20Code%20%C2%B7%20OBS-fab387?style=flat-square&labelColor=313244"><br>

<!-- ![desktop](screenshots/desktop.png) -->

</div>

## Extensions

- [User Themes](https://extensions.gnome.org/extension/19/user-themes/)
- [GlideDock](https://github.com/bquangthien25-hub/GlideDock) (installed patched by `gnome/install-glidedock.sh`)
- [Kiwi is not Apple](https://github.com/kem-a/kiwi-kemma) and [Kiwi Menu](https://github.com/kem-a/kiwi-menu)
- [Dynamic Music Pill](https://github.com/Andbal23/dynamic-music-pill)
- [Vertical Workspaces](https://github.com/G-dH/vertical-workspaces)
- [Compiz alike magic lamp effect](https://github.com/hermes83/compiz-alike-magic-lamp-effect)
- [Vitals](https://github.com/corecoding/Vitals)
- [Vicinae](https://github.com/vicinaehq/gnome-extension)

## Install

```bash
git clone https://github.com/ahadarshx/catppuccin-tahoe.git ~/rice && cd ~/rice

# theme, icons, dock
gnome/install-theme.sh
git clone --depth 1 https://github.com/vinceliuice/MacTahoe-icon-theme.git /tmp/mactahoe-icons && /tmp/mactahoe-icons/install.sh
icons/make-mactahoe-apps.sh
gnome/install-glidedock.sh

# after installing the extensions above
gnome/patch-music-pill.sh
dconf load /org/gnome/shell/extensions/ < gnome/extensions.dconf
gsettings set org.gnome.desktop.interface cursor-theme MacTahoe
gsettings set org.gnome.desktop.wm.preferences button-layout 'appmenu:minimize,maximize,close'
```

Log out and back in, then copy the rest to where it lives:

| Folder | Goes to |
|---|---|
| `zsh/zshrc`, `zsh/aliases.zsh` | `~/.zshrc`, `~/.config/zsh/` (needs oh-my-zsh) |
| `ghostty/`, `fastfetch/`, `btop/`, `cava/`, `eza/` | `~/.config/<name>/` |
| `vicinae/catppuccin-tahoe.toml`, `vicinae/settings.json` | `~/.local/share/vicinae/themes/`, `~/.config/vicinae/` |
| `wallpapers/` | `~/.local/share/backgrounds/` |
| `zen-traffic-lights/` | your Zen profile folder |
| `vscode/settings.json` | merge into VS Code settings (needs the Custom UI Style extension) |
| `obs-studio/Yami_MacTahoe.ovt` | OBS → Settings → Appearance |
| `systemd/ghostty-override.conf` | `~/.config/systemd/user/app-com.mitchellh.ghostty.service.d/override.conf`, only if Ghostty won't open from the dock |

## Tune

A small native Spotify player made for this setup (Rust, GTK4, librespot). Needs Spotify Premium.

1. Create an app at [developer.spotify.com](https://developer.spotify.com/dashboard) with redirect URI `http://127.0.0.1:8898/login`.
2. Save its client ID to `~/.config/tune/client-id`.
3. Run `Tune/install.sh`.

## Credits

[MacTahoe](https://github.com/vinceliuice/MacTahoe-gtk-theme) by vinceliuice, [Catppuccin](https://github.com/catppuccin), the extension authors above, and [librespot](https://github.com/librespot-org/librespot).
