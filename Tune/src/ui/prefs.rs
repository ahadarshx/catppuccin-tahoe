//! The Preferences, Keyboard Shortcuts and About dialogs.

use std::rc::Rc;

use adw::prelude::*;

use super::App;
use crate::backend::Quality;
use crate::tokio;

const QUALITIES: [(Quality, &str); 3] = [(Quality::Low, "Low (96 kbps)"), (Quality::Normal, "Normal (160 kbps)"), (Quality::High, "High (320 kbps)")];

impl App {
    pub(super) fn show_preferences(self: &Rc<Self>) {
        let settings = self.settings.borrow().clone();

        let quality = adw::ComboRow::builder()
            .title("Audio Quality")
            .subtitle("High needs Spotify Premium and more data")
            .model(&gtk::StringList::new(&QUALITIES.map(|(_, label)| label)))
            .selected(QUALITIES.iter().position(|(q, _)| *q == settings.quality).unwrap_or(1) as u32)
            .build();
        let normalize = adw::SwitchRow::builder()
            .title("Even Out Volume")
            .subtitle("Plays every song at about the same loudness")
            .active(settings.normalize)
            .build();
        let gapless = adw::SwitchRow::builder()
            .title("Gapless Playback")
            .subtitle("No pause between songs, for live albums and mixes")
            .active(settings.gapless)
            .build();
        let playback = adw::PreferencesGroup::builder().title("Playback").build();
        playback.add(&quality);
        playback.add(&normalize);
        playback.add(&gapless);

        let connect = adw::SwitchRow::builder()
            .title("Spotify Connect")
            .subtitle("List Tune as a speaker in Spotify on your phone and other computers")
            .active(settings.connect)
            .build();
        let background = adw::SwitchRow::builder()
            .title("Keep Playing When Closed")
            .subtitle("Closing the window leaves the music on; open Tune again to bring it back")
            .active(settings.background)
            .build();
        let general = adw::PreferencesGroup::builder().title("General").build();
        general.add(&connect);
        general.add(&background);

        let page = adw::PreferencesPage::new();
        page.add(&playback);
        page.add(&general);
        let dialog = adw::PreferencesDialog::builder().title("Preferences").build();
        dialog.add(&page);

        // Audio changes rebuild the player; what was playing carries on.
        let apply_audio = {
            let app = Rc::downgrade(self);
            let (quality, normalize, gapless) = (quality.clone(), normalize.clone(), gapless.clone());
            move || {
                let Some(app) = app.upgrade() else { return };
                app.set_settings(|s| {
                    s.quality = QUALITIES.get(quality.selected() as usize).map_or(Quality::Normal, |(q, _)| *q);
                    s.normalize = normalize.is_active();
                    s.gapless = gapless.is_active();
                });
                let audio = app.settings.borrow().audio();
                let (position, playing) = (app.clock.get().now_ms(), app.playing.get());
                glib::spawn_future_local(async move {
                    let backend = app.backend.clone();
                    let rebuilt = tokio(async move { backend.set_audio_settings(audio) }).await;
                    if rebuilt && app.loaded.get() && app.remote.borrow().is_none() {
                        app.load_current(position, playing);
                    }
                });
            }
        };
        let apply = Rc::new(apply_audio);
        quality.connect_selected_notify({
            let apply = apply.clone();
            move |_| apply()
        });
        for row in [&normalize, &gapless] {
            let apply = apply.clone();
            row.connect_active_notify(move |_| apply());
        }
        connect.connect_active_notify({
            let app = Rc::downgrade(self);
            move |row| {
                let Some(app) = app.upgrade() else { return };
                let enabled = row.is_active();
                app.set_settings(|s| s.connect = enabled);
                let backend = app.backend.clone();
                glib::spawn_future_local(async move { tokio(async move { backend.set_connect_enabled(enabled) }).await });
            }
        });
        background.connect_active_notify({
            let app = Rc::downgrade(self);
            move |row| {
                if let Some(app) = app.upgrade() {
                    app.set_settings(|s| s.background = row.is_active());
                }
            }
        });
        dialog.present(Some(&self.window));
    }

    pub(super) fn show_shortcuts(&self) {
        let sections: [(&str, &[(&str, &str)]); 3] = [
            (
                "Playback",
                &[
                    ("Play or Pause", "space"),
                    ("Next Song", "<Ctrl>Right"),
                    ("Previous Song", "<Ctrl>Left"),
                    ("Forward 10 Seconds", "<Shift>Right"),
                    ("Back 10 Seconds", "<Shift>Left"),
                    ("Volume Up", "<Ctrl>Up"),
                    ("Volume Down", "<Ctrl>Down"),
                    ("Mute", "<Ctrl>m"),
                    ("Shuffle", "<Ctrl>s"),
                    ("Repeat", "<Ctrl>r"),
                    ("Save to Liked Songs", "<Ctrl>l"),
                    ("Mini Player", "<Ctrl><Shift>m"),
                ],
            ),
            ("Navigation", &[("Search", "<Ctrl>f"), ("Now Playing", "<Ctrl>p"), ("Lyrics", "<Ctrl>y"), ("Queue", "<Ctrl>u"), ("Back", "<Alt>Left")]),
            (
                "General",
                &[("New Playlist", "<Ctrl>n"), ("Refresh Library", "F5"), ("Preferences", "<Ctrl>comma"), ("Keyboard Shortcuts", "<Ctrl>question"), ("Quit", "<Ctrl>q")],
            ),
        ];
        let dialog = adw::ShortcutsDialog::new();
        for (title, items) in sections {
            let section = adw::ShortcutsSection::new(Some(title));
            for (label, accel) in items {
                section.add(adw::ShortcutsItem::new(label, accel));
            }
            dialog.add(section);
        }
        dialog.present(Some(&self.window));
    }

    pub(super) fn show_about(&self) {
        let dialog = adw::AboutDialog::builder()
            .application_name("Tune")
            .application_icon(crate::APP_ID)
            .developer_name("Rice")
            .version(env!("CARGO_PKG_VERSION"))
            .comments("A native Spotify player for GNOME, built on librespot.\nSpotify Premium is required.")
            .build();
        dialog.present(Some(&self.window));
    }
}

use gtk::glib;
