//! The mini player: a small window that takes the main window's place.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};

use super::{App, art_image};

const ART_SIZE: i32 = 96;

pub struct MiniPlayer {
    window: adw::Window,
    art: gtk::Image,
    title: gtk::Label,
    artist: gtk::Label,
    previous: gtk::Button,
    play: gtk::Button,
    next: gtk::Button,
    seek: gtk::Scale,
}

fn control(icon: &str, tooltip: &str, classes: &[&str]) -> gtk::Button {
    gtk::Button::builder().icon_name(icon).tooltip_text(tooltip).valign(gtk::Align::Center).css_classes(classes).build()
}

impl App {
    fn build_mini(self: &Rc<Self>) -> MiniPlayer {
        let art = art_image(ART_SIZE, &["large"]);
        let title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .margin_end(32)
            .css_classes(["mini-title"])
            .build();
        let artist = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .margin_end(32)
            .css_classes(["dim-label", "caption"])
            .build();
        let previous = control("media-skip-backward-symbolic", "Previous", &["flat", "circular"]);
        let play = control("media-playback-start-symbolic", "Play", &["circular", "suggested-action"]);
        let next = control("media-skip-forward-symbolic", "Next", &["flat", "circular"]);
        let controls = gtk::Box::builder().spacing(8).halign(gtk::Align::Center).build();
        controls.append(&previous);
        controls.append(&play);
        controls.append(&next);
        let seek = gtk::Scale::builder().orientation(gtk::Orientation::Horizontal).hexpand(true).css_classes(["mini-seek"]).build();
        seek.set_range(0.0, 1.0);

        let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).hexpand(true).valign(gtk::Align::Center).build();
        text.append(&title);
        text.append(&artist);
        text.append(&controls);
        text.append(&seek);
        let content = gtk::Box::builder().spacing(14).css_classes(["mini-player"]).build();
        content.append(&art);
        content.append(&text);

        // A real header bar, so the theme's window buttons apply.
        let close = adw::HeaderBar::builder()
            .show_title(false)
            .decoration_layout(":close")
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .css_classes(["flat", "mini-header"])
            .build();
        let overlay = gtk::Overlay::builder().child(&content).build();
        overlay.add_overlay(&close);
        let handle = gtk::WindowHandle::builder().child(&overlay).build();
        let window = adw::Window::builder()
            .application(&self.application)
            .title("Tune Mini Player")
            .default_width(400)
            .resizable(false)
            .content(&handle)
            .build();
        // Added, not set: setting would drop the window's own classes, and its background.
        window.add_css_class("mini-window");

        let app = Rc::downgrade(self);
        let run = |f: fn(&Rc<App>)| {
            let app = app.clone();
            move |_: &gtk::Button| {
                if let Some(app) = app.upgrade() {
                    f(&app);
                }
            }
        };
        previous.connect_clicked(run(|app| app.previous()));
        play.connect_clicked(run(|app| app.toggle_play()));
        next.connect_clicked(run(|app| app.next()));
        seek.connect_change_value({
            let app = app.clone();
            move |_, _, value| {
                if let Some(app) = app.upgrade() {
                    app.request_seek(value);
                }
                glib::Propagation::Proceed
            }
        });
        // Closing it brings the main window back; the music carries on.
        window.connect_close_request({
            let app = app.clone();
            move |_| {
                if let Some(app) = app.upgrade() {
                    app.window.present();
                }
                glib::Propagation::Stop
            }
        });
        let shortcut = gtk::ShortcutController::new();
        shortcut.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("space"),
            Some(gtk::CallbackAction::new({
                let app = app.clone();
                move |_, _| {
                    if let Some(app) = app.upgrade() {
                        app.toggle_play();
                    }
                    glib::Propagation::Stop
                }
            })),
        ));
        window.add_controller(shortcut);

        MiniPlayer { window, art, title, artist, previous, play, next, seek }
    }

    /// Swaps the main window for the mini player, or back.
    pub(super) fn toggle_mini_player(self: &Rc<Self>) {
        if self.mini.borrow().as_ref().is_some_and(|mini| mini.window.is_visible()) {
            self.window.present();
            return;
        }
        if self.mini.borrow().is_none() {
            let mini = self.build_mini();
            self.mini.replace(Some(mini));
        }
        self.update_mini();
        self.show_mini_playing(self.playing.get());
        if let Some(mini) = &*self.mini.borrow() {
            mini.window.present();
        }
        self.window.set_visible(false);
    }

    /// The main window is back, so the mini player steps aside.
    pub(super) fn hide_mini(&self) {
        if let Some(mini) = &*self.mini.borrow() {
            mini.window.set_visible(false);
        }
    }

    pub(super) fn update_mini(self: &Rc<Self>) {
        let track = self.shown_track();
        {
            let Some(mini) = &*self.mini.borrow() else { return };
            let enabled = track.is_some();
            for button in [&mini.previous, &mini.play, &mini.next] {
                button.set_sensitive(enabled);
            }
            mini.seek.set_sensitive(enabled);
            mini.title.set_label(track.as_ref().map_or("Not playing", |t| &t.name));
            mini.title.set_tooltip_text(track.as_ref().map(|t| t.name.as_str()));
            mini.artist.set_label(track.as_ref().map_or("", |t| &t.artists));
            mini.seek.set_range(0.0, f64::from(track.as_ref().map_or(1, |t| t.duration_ms.max(1))));
            mini.seek.set_value(f64::from(self.clock.get().now_ms()));
            mini.art.set_paintable(gdk::Paintable::NONE);
            mini.art.set_icon_name(Some("audio-x-generic-symbolic"));
            mini.art.add_css_class("placeholder");
        }
        let Some(track) = track else { return };
        let Some(url) = track.image_large.clone().or(track.image_small.clone()) else { return };
        let Some(art) = self.mini.borrow().as_ref().map(|mini| mini.art.clone()) else { return };
        let uri = track.uri;
        let app = Rc::downgrade(self);
        self.load_image_if(&art, url, ART_SIZE, move || {
            app.upgrade().and_then(|app| app.shown_track()).is_some_and(|t| t.uri == uri)
        });
    }

    pub(super) fn show_mini_playing(&self, playing: bool) {
        if let Some(mini) = &*self.mini.borrow() {
            let (icon, tooltip) = if playing { ("media-playback-pause-symbolic", "Pause") } else { ("media-playback-start-symbolic", "Play") };
            mini.play.set_icon_name(icon);
            mini.play.set_tooltip_text(Some(tooltip));
        }
    }

    pub(super) fn show_mini_position(&self, position_ms: u32) {
        if let Some(mini) = &*self.mini.borrow() {
            mini.seek.set_value(f64::from(position_ms));
        }
    }

    /// With nothing to play: called from `reset_bar`, which has no `Rc`.
    pub(super) fn clear_mini(&self) {
        if let Some(mini) = &*self.mini.borrow() {
            mini.title.set_label("Not playing");
            mini.artist.set_label("");
            mini.seek.set_value(0.0);
            for button in [&mini.previous, &mini.play, &mini.next] {
                button.set_sensitive(false);
            }
            mini.art.set_paintable(gdk::Paintable::NONE);
            mini.art.set_icon_name(Some("audio-x-generic-symbolic"));
            mini.art.add_css_class("placeholder");
        }
    }
}
