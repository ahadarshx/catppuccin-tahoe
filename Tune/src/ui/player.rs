//! The player bar and everything behind it: the queue, playback, saving
//! what was playing, media controls (MPRIS), Spotify Connect, and the Now
//! Playing and Queue pages.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, glib};

use super::{App, art_image, format_time};
use crate::api::Track;
use crate::backend::{ConnectUpdate, PlayerUpdate};
use crate::settings::{Repeat, Settings};
use crate::{mpris, tokio};

/// Going back within this many milliseconds of a song's start goes to the
/// previous song; later, it restarts the current one.
const RESTART_THRESHOLD_MS: u32 = 3000;
const BAR_ART_SIZE: i32 = 56;
/// Space kept free under scrolling pages for the floating player.
pub const CLEARANCE: i32 = 112;
/// The same, when the player stacks in a narrow window.
pub const CLEARANCE_STACKED: i32 = 196;
/// Songs shown after the current one on the Queue page.
const QUEUE_PREVIEW: usize = 50;
/// While playing, the position is saved this often (in 250ms ticks).
const SAVE_EVERY_TICKS: u32 = 60;

thread_local! {
    static CLEARANCE_NOW: Cell<i32> = const { Cell::new(CLEARANCE) };
}

pub fn clearance_height() -> i32 {
    CLEARANCE_NOW.with(Cell::get)
}

pub fn set_clearance_height(height: i32) {
    CLEARANCE_NOW.with(|c| c.set(height));
}

/// The songs being played, and the order they play in: the list order, or
/// a shuffled one. Songs queued by hand play first, between list songs.
#[derive(Default)]
pub struct Queue {
    tracks: Vec<Track>,
    order: Vec<usize>,
    position: usize,
    /// Play Next and Add to Queue, in the order they will play.
    upcoming: VecDeque<Track>,
    /// A queued song playing now; the list waits at `position`.
    interlude: Option<Track>,
    /// What the list is ("Liked Songs"), for the Queue page.
    source: String,
}

impl Queue {
    fn new(tracks: Vec<Track>, start: usize, shuffle: bool, source: &str, upcoming: VecDeque<Track>) -> Self {
        let mut queue = Self {
            order: (0..tracks.len()).collect(),
            tracks,
            position: start,
            upcoming,
            interlude: None,
            source: source.to_owned(),
        };
        if shuffle {
            queue.shuffle();
        }
        queue
    }

    fn current(&self) -> Option<&Track> {
        self.interlude.as_ref().or_else(|| self.tracks.get(*self.order.get(self.position)?))
    }

    /// The current song first, then everything else in random order.
    fn shuffle(&mut self) {
        let Some(&current) = self.order.get(self.position) else { return };
        let mut rest: Vec<usize> = (0..self.tracks.len()).filter(|&i| i != current).collect();
        for i in (1..rest.len()).rev() {
            let j = glib::random_int_range(0, i as i32 + 1) as usize;
            rest.swap(i, j);
        }
        self.order = std::iter::once(current).chain(rest).collect();
        self.position = 0;
    }

    /// Back to list order, staying on the current song.
    fn unshuffle(&mut self) {
        let current = self.order.get(self.position).copied().unwrap_or(0);
        self.order = (0..self.tracks.len()).collect();
        self.position = current;
    }

    /// On to the next song: a queued one, else the list's next; `wrap`
    /// starts the list over at its end. False when there is nothing next.
    fn advance(&mut self, wrap: bool) -> bool {
        if let Some(track) = self.upcoming.pop_front() {
            self.interlude = Some(track);
            return true;
        }
        self.interlude = None;
        if self.position + 1 < self.order.len() {
            self.position += 1;
            true
        } else if wrap && !self.order.is_empty() {
            self.position = 0;
            true
        } else {
            false
        }
    }

    fn back(&mut self) -> bool {
        if self.interlude.take().is_some() {
            return true;
        }
        if self.position > 0 {
            self.position -= 1;
            true
        } else {
            false
        }
    }

    fn next_in_list(&self, count: usize) -> Vec<Track> {
        self.order.iter().skip(self.position + 1).take(count).filter_map(|&i| self.tracks.get(i).cloned()).collect()
    }
}

/// What is restored on the next start: the queue and where it was.
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedState {
    tracks: Vec<Track>,
    order: Vec<usize>,
    position: usize,
    position_ms: u32,
    shuffle: bool,
    #[serde(default)]
    upcoming: Vec<Track>,
    #[serde(default)]
    interlude: Option<Track>,
    #[serde(default)]
    source: String,
}

fn state_path() -> std::path::PathBuf {
    glib::user_data_dir().join("tune").join("last-played.json")
}

/// Where playback is, without asking the player: the position at the last
/// event, plus the time since then while playing.
#[derive(Default, Clone, Copy)]
pub struct Clock {
    base_ms: u32,
    since: Option<Instant>,
}

impl Clock {
    pub fn now_ms(&self) -> u32 {
        let elapsed = self.since.map_or(0, |since| since.elapsed().as_millis() as u32);
        self.base_ms.saturating_add(elapsed)
    }
}

/// Another Spotify app is playing through Tune.
pub struct Remote {
    track: Option<Track>,
    /// Where Tune's own song was, to show again afterwards.
    local_ms: u32,
}

pub struct PlayerBar {
    pub root: gtk::CenterBox,
    pub now_playing: gtk::Box,
    cover: gtk::Button,
    art: gtk::Image,
    title: gtk::Label,
    artist: gtk::Label,
    connect_badge: gtk::Image,
    like: gtk::ToggleButton,
    pub shuffle: gtk::ToggleButton,
    previous: gtk::Button,
    play: gtk::Button,
    next: gtk::Button,
    repeat: gtk::Button,
    position: gtk::Label,
    duration: gtk::Label,
    seek: gtk::Scale,
    queue: gtk::ToggleButton,
    lyrics: gtk::ToggleButton,
    pub mute: gtk::ToggleButton,
    pub volume: gtk::Scale,
    pub volume_box: gtk::Box,
    pub right: gtk::Box,
}

pub fn build_player_bar(settings: &Settings) -> PlayerBar {
    let art = art_image(BAR_ART_SIZE, &[]);
    let cover = gtk::Button::builder().child(&art).tooltip_text("Now Playing").valign(gtk::Align::Center).css_classes(["cover-button"]).build();
    let title = gtk::Label::builder()
        .label("Not playing")
        .xalign(0.0)
        .max_width_chars(26)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["title", "links"])
        .build();
    let connect_badge = gtk::Image::builder()
        .icon_name("network-wireless-symbolic")
        .tooltip_text("Playing from another Spotify app through Spotify Connect")
        .css_classes(["connect-badge"])
        .visible(false)
        .build();
    let title_line = gtk::Box::builder().spacing(6).hexpand(true).build();
    title.set_hexpand(true);
    title_line.append(&title);
    title_line.append(&connect_badge);
    let artist = gtk::Label::builder()
        .xalign(0.0)
        .max_width_chars(26)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["dim-label", "caption", "links"])
        .build();
    let labels = gtk::Box::builder().orientation(gtk::Orientation::Vertical).valign(gtk::Align::Center).spacing(3).hexpand(true).build();
    labels.append(&title_line);
    labels.append(&artist);
    let like = gtk::ToggleButton::builder()
        .icon_name("tune-heart-symbolic")
        .tooltip_text("Save to Liked Songs")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular", "like-button"])
        .sensitive(false)
        .build();
    let now_playing = gtk::Box::builder().spacing(14).margin_end(24).build();
    now_playing.append(&cover);
    now_playing.append(&labels);
    now_playing.append(&like);

    let button = |icon: &str, tooltip: &str| {
        gtk::Button::builder().icon_name(icon).tooltip_text(tooltip).valign(gtk::Align::Center).css_classes(["flat", "circular"]).sensitive(false).build()
    };
    let shuffle = gtk::ToggleButton::builder()
        .icon_name("media-playlist-shuffle-symbolic")
        .tooltip_text("Shuffle")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    let previous = button("media-skip-backward-symbolic", "Previous");
    let play = gtk::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .tooltip_text("Play")
        .valign(gtk::Align::Center)
        .css_classes(["circular", "suggested-action"])
        .sensitive(false)
        .build();
    let next = button("media-skip-forward-symbolic", "Next");
    let repeat = gtk::Button::builder().valign(gtk::Align::Center).css_classes(["flat", "circular"]).build();
    let controls = gtk::Box::builder().spacing(12).halign(gtk::Align::Center).build();
    controls.append(&shuffle);
    controls.append(&previous);
    controls.append(&play);
    controls.append(&next);
    controls.append(&repeat);

    let time = |text: &str| gtk::Label::builder().label(text).width_chars(5).css_classes(["dim-label", "caption", "track-duration"]).build();
    let position = time("0:00");
    let duration = time("0:00");
    let seek = gtk::Scale::builder().hexpand(true).sensitive(false).css_classes(["flat-slider"]).build();
    seek.set_range(0.0, 1.0);
    let timeline = gtk::Box::builder().spacing(10).build();
    timeline.append(&position);
    timeline.append(&seek);
    timeline.append(&duration);

    let center = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).valign(gtk::Align::Center).spacing(4).build();
    center.append(&controls);
    center.append(&timeline);

    let queue = gtk::ToggleButton::builder()
        .icon_name("view-list-symbolic")
        .tooltip_text("Queue")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    let lyrics = gtk::ToggleButton::builder()
        .hexpand(true)
        .halign(gtk::Align::End)
        .icon_name("tune-lyrics-symbolic")
        .tooltip_text("Lyrics")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    let mute = gtk::ToggleButton::builder()
        .tooltip_text("Mute")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .active(settings.muted)
        .build();
    let volume = gtk::Scale::builder().width_request(110).valign(gtk::Align::Center).css_classes(["flat-slider"]).build();
    volume.set_range(0.0, 1.0);
    volume.set_increments(0.05, 0.1);
    volume.set_value(settings.volume);
    let volume_box = gtk::Box::builder().spacing(4).valign(gtk::Align::Center).build();
    volume_box.append(&mute);
    volume_box.append(&volume);
    let mini = gtk::Button::builder()
        .icon_name("view-restore-symbolic")
        .tooltip_text("Mini Player")
        .action_name("app.mini-player")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    let right = gtk::Box::builder().spacing(6).margin_start(24).valign(gtk::Align::Center).build();
    right.append(&lyrics);
    right.append(&queue);
    right.append(&mini);
    right.append(&volume_box);

    // The controls stay in the true middle, whatever the song's name.
    let root = gtk::CenterBox::builder().css_classes(["player-bar"]).build();
    root.set_start_widget(Some(&now_playing));
    root.set_center_widget(Some(&center));
    root.set_end_widget(Some(&right));

    let bar = PlayerBar {
        root,
        now_playing,
        cover,
        art,
        title,
        artist,
        connect_badge,
        like,
        shuffle,
        previous,
        play,
        next,
        repeat,
        position,
        duration,
        seek,
        queue,
        lyrics,
        mute,
        volume,
        volume_box,
        right,
    };
    show_repeat(&bar, settings.repeat);
    show_volume_icon(&bar);
    bar
}

fn show_repeat(bar: &PlayerBar, repeat: Repeat) {
    let (icon, tooltip) = match repeat {
        Repeat::Off => ("media-playlist-repeat-symbolic", "Repeat"),
        Repeat::All => ("media-playlist-repeat-symbolic", "Repeat All"),
        Repeat::One => ("media-playlist-repeat-song-symbolic", "Repeat One"),
    };
    bar.repeat.set_icon_name(icon);
    bar.repeat.set_tooltip_text(Some(tooltip));
    if repeat == Repeat::Off {
        bar.repeat.remove_css_class("on");
    } else {
        bar.repeat.add_css_class("on");
    }
}

fn show_volume_icon(bar: &PlayerBar) {
    let level = bar.volume.value();
    let icon = match () {
        _ if bar.mute.is_active() || level <= 0.0 => "audio-volume-muted-symbolic",
        _ if level < 0.34 => "audio-volume-low-symbolic",
        _ if level < 0.67 => "audio-volume-medium-symbolic",
        _ => "audio-volume-high-symbolic",
    };
    bar.mute.set_icon_name(icon);
    bar.mute.set_tooltip_text(Some(if bar.mute.is_active() { "Unmute" } else { "Mute" }));
}

/// The cover's most colourful tone, for tinting the player.
fn cover_tint(texture: &gdk::Texture) -> Option<(u8, u8, u8)> {
    let mut downloader = gdk::TextureDownloader::new(texture);
    downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    let (width, height) = (texture.width() as usize, texture.height() as usize);
    let step = (width.max(height) / 48).max(1);
    let (mut r, mut g, mut b, mut total) = (0.0, 0.0, 0.0, 0.0);
    for y in (0..height).step_by(step) {
        for x in (0..width).step_by(step) {
            let i = y * stride + x * 4;
            let Some(px) = bytes.get(i..i + 3) else { continue };
            let (pr, pg, pb) = (f64::from(px[0]) / 255.0, f64::from(px[1]) / 255.0, f64::from(px[2]) / 255.0);
            let max = pr.max(pg).max(pb);
            let min = pr.min(pg).min(pb);
            let saturation = if max > 0.0 { (max - min) / max } else { 0.0 };
            // Vivid, mid-bright pixels count most; greys and near-blacks barely.
            let weight = saturation * saturation * (1.0 - (max - 0.6).abs()) + 0.02;
            r += pr * weight;
            g += pg * weight;
            b += pb * weight;
            total += weight;
        }
    }
    if total <= 0.0 {
        return None;
    }
    let (r, g, b) = (r / total, g / total, b / total);
    // Bright enough to show on a dark bar, never glaring.
    let max = r.max(g).max(b).max(0.01);
    let scale = (0.75 / max).min(2.5);
    let channel = |c: f64| ((c * scale).clamp(0.0, 1.0) * 255.0) as u8;
    Some((channel(r), channel(g), channel(b)))
}

/// The big cover view.
pub struct NowPlaying {
    page: adw::NavigationPage,
    backdrop: gtk::Picture,
    art: gtk::Picture,
    title: gtk::Label,
    artist: gtk::Label,
    album: gtk::Label,
}

pub struct QueuePage {
    page: adw::NavigationPage,
    content: adw::Bin,
}

impl App {
    pub(super) fn connect_player_bar(self: &Rc<Self>) {
        let bar = &self.bar;
        bar.play.connect_clicked({
            let app = self.clone();
            move |_| app.toggle_play()
        });
        bar.next.connect_clicked({
            let app = self.clone();
            move |_| app.next()
        });
        bar.previous.connect_clicked({
            let app = self.clone();
            move |_| app.previous()
        });
        bar.shuffle.connect_toggled({
            let app = self.clone();
            move |button| {
                if !app.syncing.get() {
                    app.set_shuffle(button.is_active());
                }
            }
        });
        bar.repeat.connect_clicked({
            let app = self.clone();
            move |_| app.cycle_repeat()
        });
        bar.like.connect_toggled({
            let app = self.clone();
            move |button| {
                if !app.syncing.get()
                    && let Some(track) = app.shown_track()
                {
                    app.set_song_saved(track, button.is_active());
                }
            }
        });
        bar.cover.connect_clicked({
            let app = self.clone();
            move |_| app.toggle_now_playing()
        });
        bar.lyrics.connect_clicked({
            let app = self.clone();
            move |_| app.toggle_lyrics()
        });
        bar.queue.connect_clicked({
            let app = self.clone();
            move |_| app.toggle_queue_page()
        });
        for label in [&bar.title, &bar.artist] {
            let app = Rc::downgrade(self);
            label.connect_activate_link(move |_, link| app.upgrade().map_or(glib::Propagation::Proceed, |app| app.open_link(link)));
        }
        // Only user changes arrive here; the ticker moves the slider silently.
        bar.seek.connect_change_value({
            let app = self.clone();
            move |_, _, value| {
                app.request_seek(value);
                glib::Propagation::Proceed
            }
        });
        bar.volume.connect_value_changed({
            let app = self.clone();
            move |scale| {
                if app.syncing.get() {
                    return;
                }
                app.settings.borrow_mut().volume = scale.value();
                // Dragging the volume up unmutes, as on a Mac.
                if app.bar.mute.is_active() && scale.value() > 0.0 {
                    app.bar.mute.set_active(false);
                }
                app.apply_volume();
            }
        });
        bar.mute.connect_toggled({
            let app = self.clone();
            move |button| {
                app.settings.borrow_mut().muted = button.is_active();
                app.apply_volume();
            }
        });
        // Scrolling over the volume control changes it.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll({
            let app = self.clone();
            move |_, _, dy| {
                app.bar.volume.set_value(app.bar.volume.value() - dy * 0.05);
                glib::Propagation::Stop
            }
        });
        bar.mute.add_controller(scroll);
        self.apply_volume();
        // Keep the Queue and Now Playing buttons in step with the pages.
        self.nav.connect_visible_page_notify({
            let app = Rc::downgrade(self);
            move |nav| {
                let Some(app) = app.upgrade() else { return };
                let visible = nav.visible_page();
                let showing = |page: Option<adw::NavigationPage>| page.is_some() && page == visible;
                app.bar.queue.set_active(showing(app.queue_page.borrow().as_ref().map(|q| q.page.clone())));
                app.bar.lyrics.set_active(showing(app.lyrics.borrow().as_ref().map(|l| l.page())));
                if showing(app.queue_page.borrow().as_ref().map(|q| q.page.clone())) {
                    app.refresh_queue_page();
                }
            }
        });
    }

    fn apply_volume(&self) {
        let volume = if self.bar.mute.is_active() { 0.0 } else { self.bar.volume.value() };
        // Squared: the slider's middle sounds like the middle.
        self.backend.set_volume(volume.powi(2));
        show_volume_icon(&self.bar);
        if let Some(player) = self.mpris.borrow().clone() {
            glib::spawn_future_local(async move {
                let _ = player.set_volume(volume).await;
            });
        }
    }

    /// Whether there is a song in the player bar.
    pub(super) fn has_track(&self) -> bool {
        self.shown_track().is_some()
    }

    /// The song in the player bar: another app's over Spotify Connect, or Tune's own.
    pub(super) fn shown_track(&self) -> Option<Track> {
        match &*self.remote.borrow() {
            Some(remote) => remote.track.clone(),
            None => self.current.borrow().clone(),
        }
    }

    // The queue

    pub(super) fn play_list(self: &Rc<Self>, tracks: Vec<Track>, index: usize, source: &str) {
        if index >= tracks.len() {
            return;
        }
        let upcoming = std::mem::take(&mut self.queue.borrow_mut().upcoming);
        self.queue.replace(Queue::new(tracks, index, self.bar.shuffle.is_active(), source, upcoming));
        self.play_current();
    }

    /// Play Next (`next`) or Add to Queue. With nothing playing, it plays.
    pub(super) fn queue_song(self: &Rc<Self>, track: Track, next: bool) {
        if self.current.borrow().is_none() {
            self.play_list(vec![track], 0, "");
            return;
        }
        {
            let mut queue = self.queue.borrow_mut();
            if next {
                queue.upcoming.push_front(track.clone());
            } else {
                queue.upcoming.push_back(track.clone());
            }
        }
        self.toast(&format!("“{}” {}", track.name, if next { "plays next" } else { "added to the queue" }));
        self.save_state();
        self.refresh_queue_page();
    }

    pub(super) fn set_shuffle(self: &Rc<Self>, shuffle: bool) {
        if self.remote.borrow().is_some() {
            self.backend.connect_shuffle(shuffle);
            return;
        }
        {
            let mut queue = self.queue.borrow_mut();
            if shuffle { queue.shuffle() } else { queue.unshuffle() }
        }
        self.refresh_queue_page();
        if let Some(player) = self.mpris.borrow().clone() {
            glib::spawn_future_local(async move {
                let _ = player.set_shuffle(shuffle).await;
            });
        }
    }

    pub(super) fn cycle_repeat(self: &Rc<Self>) {
        let repeat = self.settings.borrow().repeat.next();
        self.set_settings(|settings| settings.repeat = repeat);
        show_repeat(&self.bar, repeat);
        if self.remote.borrow().is_some() {
            self.backend.connect_repeat(repeat == Repeat::All, repeat == Repeat::One);
        }
        if let Some(player) = self.mpris.borrow().clone() {
            glib::spawn_future_local(async move { mpris::set_repeat(&player, repeat).await });
        }
    }

    fn play_current(self: &Rc<Self>) {
        let Some(track) = self.queue.borrow().current().cloned() else { return };
        // Tune plays its own song: any Connect session is over.
        self.remote.replace(None);
        self.current.replace(Some(track.clone()));
        self.set_playing(false, 0);
        self.show_track(&track);
        self.load_current(0, true);
        self.save_state();
        self.refresh_queue_page();
    }

    /// Hands the current song to the player at `position_ms`.
    pub(super) fn load_current(self: &Rc<Self>, position_ms: u32, play: bool) {
        let Some(track) = self.current.borrow().clone() else { return };
        self.loaded.set(true);
        let app = self.clone();
        glib::spawn_future_local(async move {
            let backend = app.backend.clone();
            let uri = track.uri.clone();
            if let Err(e) = tokio(async move { backend.load(uri, position_ms, play).await }).await {
                app.toast(&format!("Could not play “{}”: {e}", track.name));
            }
        });
    }

    pub(super) fn save_state(&self) {
        self.settings.borrow().save();
        let queue = self.queue.borrow();
        if queue.tracks.is_empty() {
            let _ = std::fs::remove_file(state_path());
            return;
        }
        let position_ms = match &*self.remote.borrow() {
            Some(remote) => remote.local_ms,
            None => self.clock.get().now_ms(),
        };
        let state = SavedState {
            tracks: queue.tracks.clone(),
            order: queue.order.clone(),
            position: queue.position,
            position_ms,
            shuffle: self.bar.shuffle.is_active(),
            upcoming: queue.upcoming.iter().cloned().collect(),
            interlude: queue.interlude.clone(),
            source: queue.source.clone(),
        };
        let path = state_path();
        let result = std::fs::create_dir_all(path.parent().expect("parent"))
            .map_err(|e| e.to_string())
            .and_then(|()| serde_json::to_vec(&state).map_err(|e| e.to_string()))
            .and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()));
        if let Err(e) = result {
            log::warn!("could not save what was playing: {e}");
        }
    }

    /// Last session's song, shown paused where it was left; play resumes there.
    pub(super) fn restore_state(self: &Rc<Self>) {
        let Ok(json) = std::fs::read(state_path()) else { return };
        let Ok(state) = serde_json::from_slice::<SavedState>(&json) else { return };
        let valid = state.order.len() == state.tracks.len()
            && state.order.iter().all(|&i| i < state.tracks.len())
            && state.position < state.order.len();
        if !valid {
            return;
        }
        self.syncing.set(true);
        self.bar.shuffle.set_active(state.shuffle);
        self.syncing.set(false);
        self.queue.replace(Queue {
            tracks: state.tracks,
            order: state.order,
            position: state.position,
            upcoming: state.upcoming.into(),
            interlude: state.interlude,
            source: state.source,
        });
        let Some(track) = self.queue.borrow().current().cloned() else { return };

        self.current.replace(Some(track.clone()));
        self.loaded.set(false);
        self.show_track(&track);
        let position = state.position_ms.min(track.duration_ms);
        self.set_playing(false, position);
        self.bar.seek.set_value(f64::from(position));
        self.bar.position.set_label(&format_time(position));
    }

    // The player bar

    pub(super) fn show_track(self: &Rc<Self>, track: &Track) {
        let bar = &self.bar;
        bar.title.set_markup(&super::album_link(track, &track.name));
        bar.title.set_tooltip_text(Some(&track.name));
        bar.artist.set_markup(&super::artist_links(track));
        bar.duration.set_label(&format_time(track.duration_ms));
        bar.position.set_label("0:00");
        bar.seek.set_range(0.0, f64::from(track.duration_ms.max(1)));
        bar.seek.set_value(0.0);
        for widget in [bar.play.upcast_ref::<gtk::Widget>(), bar.next.upcast_ref(), bar.previous.upcast_ref(), bar.seek.upcast_ref()] {
            widget.set_sensitive(true);
        }
        bar.connect_badge.set_visible(self.remote.borrow().is_some());
        bar.art.set_icon_name(Some("audio-x-generic-symbolic"));
        bar.art.add_css_class("placeholder");
        if let Some(url) = track.image_small.clone() {
            let uri = track.uri.clone();
            let app = Rc::downgrade(self);
            self.load_image_if(&bar.art, url, BAR_ART_SIZE, move || {
                app.upgrade().and_then(|app| app.shown_track()).is_some_and(|t| t.uri == uri)
            });
        }
        self.tint_from(track);
        self.window.set_title(Some(&format!("{} – {}", track.name, track.artists)));
        self.mark_now_playing(Some(&track.uri));
        self.refresh_liked(track);
        self.update_now_playing();
        self.update_mini();
        self.load_lyrics();

        if let Some(player) = self.mpris.borrow().clone() {
            let track = track.clone();
            glib::spawn_future_local(async move { mpris::set_track(&player, &track).await });
        }
    }

    pub(super) fn reset_bar(&self) {
        let bar = &self.bar;
        bar.title.set_label("Not playing");
        bar.artist.set_label("");
        bar.position.set_label("0:00");
        bar.duration.set_label("0:00");
        bar.seek.set_value(0.0);
        bar.connect_badge.set_visible(false);
        for widget in [bar.play.upcast_ref::<gtk::Widget>(), bar.next.upcast_ref(), bar.previous.upcast_ref(), bar.seek.upcast_ref()] {
            widget.set_sensitive(false);
        }
        bar.art.set_paintable(gdk::Paintable::NONE);
        bar.art.set_icon_name(Some("audio-x-generic-symbolic"));
        bar.art.add_css_class("placeholder");
        self.show_liked(None);
        self.cover_css.load_from_string("");
        self.window.set_title(Some("Tune"));
        self.mark_now_playing(None);
        self.clear_mini();
    }

    /// Tints the player bar and Now Playing with the cover's colour.
    fn tint_from(self: &Rc<Self>, track: &Track) {
        let Some(url) = track.image_small.clone() else {
            self.cover_css.load_from_string("");
            return;
        };
        let app = self.clone();
        let uri = track.uri.clone();
        glib::spawn_future_local(async move {
            let Some(texture) = app.texture(url).await else { return };
            if app.shown_track().is_none_or(|t| t.uri != uri) {
                return;
            }
            let css = match cover_tint(&texture) {
                Some((r, g, b)) => format!(
                    ".player-bar {{ background-color: color-mix(in srgb, var(--popover-bg-color) 80%, rgb({r},{g},{b})); }}
                     .now-playing-view {{ background-color: color-mix(in srgb, var(--window-bg-color) 72%, rgb({r},{g},{b})); }}"
                ),
                None => String::new(),
            };
            app.cover_css.load_from_string(&css);
        });
    }

    fn refresh_liked(self: &Rc<Self>, track: &Track) {
        self.show_liked(None);
        let app = self.clone();
        let uri = track.uri.clone();
        glib::spawn_future_local(async move {
            let backend = app.backend.clone();
            let ask = uri.clone();
            let saved = tokio(async move { backend.is_saved(ask).await }).await;
            if app.shown_track().is_some_and(|t| t.uri == uri) {
                match saved {
                    Ok(saved) => app.show_liked(Some(saved)),
                    Err(e) => log::warn!("liked songs: {e}"),
                }
            }
        });
    }

    /// The heart: filled when saved, greyed out while unknown.
    pub(super) fn show_liked(&self, saved: Option<bool>) {
        self.liked.set(saved);
        let like = &self.bar.like;
        self.syncing.set(true);
        like.set_active(saved == Some(true));
        self.syncing.set(false);
        like.set_sensitive(saved.is_some());
        if saved == Some(true) {
            like.set_icon_name("tune-heart-filled-symbolic");
            like.set_tooltip_text(Some("Remove from Liked Songs"));
            like.add_css_class("liked");
        } else {
            like.set_icon_name("tune-heart-symbolic");
            like.set_tooltip_text(Some("Save to Liked Songs"));
            like.remove_css_class("liked");
        }
    }

    pub(super) fn toggle_like(&self) {
        if self.liked.get().is_some() {
            self.bar.like.set_active(!self.bar.like.is_active());
        }
    }

    // Playback

    pub(super) fn toggle_play(self: &Rc<Self>) {
        if self.remote.borrow().is_some() {
            self.backend.connect_play_pause();
            return;
        }
        if self.current.borrow().is_none() {
            return;
        }
        if !self.loaded.get() {
            self.load_current(self.clock.get().now_ms(), true);
        } else if self.playing.get() {
            self.backend.pause();
        } else {
            self.backend.play();
        }
    }

    pub(super) fn next(self: &Rc<Self>) {
        if self.remote.borrow().is_some() {
            self.backend.connect_next();
            return;
        }
        let wrap = self.settings.borrow().repeat != Repeat::Off;
        let moved = self.queue.borrow_mut().advance(wrap);
        if moved {
            self.play_current();
        }
    }

    pub(super) fn previous(self: &Rc<Self>) {
        if self.remote.borrow().is_some() {
            self.backend.connect_previous();
            return;
        }
        if self.clock.get().now_ms() > RESTART_THRESHOLD_MS {
            self.seek_to(0);
            return;
        }
        let moved = self.queue.borrow_mut().back();
        if moved {
            self.play_current();
        } else {
            self.seek_to(0);
        }
    }

    pub(super) fn seek_to(self: &Rc<Self>, position_ms: u32) {
        if self.remote.borrow().is_some() {
            self.backend.connect_seek(position_ms);
        } else if self.loaded.get() {
            self.backend.seek(position_ms);
        } else {
            self.clock.set(Clock { base_ms: position_ms, since: None });
            self.bar.seek.set_value(f64::from(position_ms));
            self.bar.position.set_label(&format_time(position_ms));
        }
    }

    pub(super) fn seek_by(self: &Rc<Self>, delta_ms: i64) {
        let Some(track) = self.shown_track() else { return };
        let target = (i64::from(self.clock.get().now_ms()) + delta_ms).clamp(0, i64::from(track.duration_ms.saturating_sub(1000)));
        self.seek_to(target as u32);
    }

    /// Dragging the slider sends a burst of values; seek once it settles.
    pub(super) fn request_seek(self: &Rc<Self>, value: f64) {
        if let Some(source) = self.pending_seek.take() {
            source.remove();
        }
        self.bar.position.set_label(&format_time(value as u32));
        let app = self.clone();
        let source = glib::timeout_add_local_once(Duration::from_millis(180), move || {
            app.pending_seek.replace(None);
            app.seek_to(value as u32);
        });
        self.pending_seek.replace(Some(source));
    }

    pub(super) fn set_playing(&self, playing: bool, position_ms: u32) {
        self.playing.set(playing);
        self.clock.set(Clock { base_ms: position_ms, since: playing.then(Instant::now) });
        let (icon, tooltip) = if playing { ("media-playback-pause-symbolic", "Pause") } else { ("media-playback-start-symbolic", "Play") };
        self.bar.play.set_icon_name(icon);
        self.bar.play.set_tooltip_text(Some(tooltip));
        self.show_mini_playing(playing);
        if let Some(player) = self.mpris.borrow().clone() {
            glib::spawn_future_local(async move { mpris::set_playing(&player, playing).await });
        }
    }

    pub(super) fn on_player_update(self: &Rc<Self>, update: PlayerUpdate) {
        if let PlayerUpdate::Connect(update) = update {
            self.on_connect_update(update);
            return;
        }
        // While another app plays through Tune, Tune's own player is paused
        // and its events are not what the bar shows.
        if self.remote.borrow().is_some() {
            return;
        }
        let current = self.current.borrow().as_ref().map(|t| t.uri.clone());
        let is_current = |uri: &str| current.as_deref() == Some(uri);
        match update {
            PlayerUpdate::Playing { uri, position_ms } if is_current(&uri) => {
                self.failures.set(0);
                self.set_playing(true, position_ms);
            }
            PlayerUpdate::Paused { uri, position_ms } if is_current(&uri) => {
                self.set_playing(false, position_ms);
                self.save_state();
            }
            PlayerUpdate::Seeked { uri, position_ms } if is_current(&uri) => {
                self.clock.set(Clock { base_ms: position_ms, since: self.playing.get().then(Instant::now) });
                self.bar.seek.set_value(f64::from(position_ms));
                self.bar.position.set_label(&format_time(position_ms));
                if let Some(player) = self.mpris.borrow().clone() {
                    glib::spawn_future_local(async move {
                        let _ = player.seeked(mpris_server::Time::from_millis(i64::from(position_ms))).await;
                    });
                }
            }
            PlayerUpdate::EndOfTrack { uri } if is_current(&uri) && !self.has_played() => self.on_failed_track(),
            PlayerUpdate::EndOfTrack { uri } if is_current(&uri) && self.sleep_at_end.get() => {
                self.set_sleep_timer("off");
                self.set_playing(false, 0);
                self.save_state();
                self.toast("Sleep timer: stopped at the end of the song");
            }
            PlayerUpdate::EndOfTrack { uri } if is_current(&uri) => {
                let repeat = self.settings.borrow().repeat;
                if repeat == Repeat::One {
                    self.load_current(0, true);
                    return;
                }
                let moved = self.queue.borrow_mut().advance(repeat == Repeat::All);
                if moved {
                    self.play_current();
                } else {
                    self.set_playing(false, 0);
                    self.save_state();
                }
            }
            PlayerUpdate::Unavailable { uri } if is_current(&uri) => self.on_failed_track(),
            _ => {}
        }
    }

    /// Another Spotify app playing through Tune: the bar follows it, and
    /// its buttons control it, until Tune plays something itself.
    fn on_connect_update(self: &Rc<Self>, update: ConnectUpdate) {
        let entering = self.remote.borrow().is_none();
        match update {
            ConnectUpdate::Inactive => {
                let Some(remote) = self.remote.take() else { return };
                match self.current.borrow().clone() {
                    Some(track) => {
                        self.show_track(&track);
                        self.set_playing(false, remote.local_ms);
                        self.bar.seek.set_value(f64::from(remote.local_ms));
                        self.bar.position.set_label(&format_time(remote.local_ms));
                    }
                    None => {
                        self.set_playing(false, 0);
                        self.reset_bar();
                    }
                }
                self.refresh_queue_page();
                return;
            }
            ConnectUpdate::Volume(volume) => {
                self.syncing.set(true);
                self.bar.volume.set_value(volume.sqrt());
                self.syncing.set(false);
                self.settings.borrow_mut().volume = volume.sqrt();
                show_volume_icon(&self.bar);
                return;
            }
            _ => {}
        }
        if entering {
            let local_ms = self.clock.get().now_ms();
            if self.playing.get() {
                self.backend.pause();
            }
            self.save_state();
            self.remote.replace(Some(Remote { track: None, local_ms }));
            self.toast("Playing from another Spotify app");
            self.refresh_queue_page();
        }
        match update {
            ConnectUpdate::Track(track) => {
                if let Some(remote) = self.remote.borrow_mut().as_mut() {
                    remote.track = Some(track.clone());
                }
                self.show_track(&track);
                self.set_playing(self.playing.get(), 0);
            }
            ConnectUpdate::Playing { position_ms } => self.set_playing(true, position_ms),
            ConnectUpdate::Paused { position_ms } => self.set_playing(false, position_ms),
            ConnectUpdate::Seeked { position_ms } => {
                self.clock.set(Clock { base_ms: position_ms, since: self.playing.get().then(Instant::now) });
                self.bar.seek.set_value(f64::from(position_ms));
            }
            ConnectUpdate::Inactive | ConnectUpdate::Volume(_) => {}
        }
    }

    /// Whether the current song started at all, as opposed to failing to load.
    fn has_played(&self) -> bool {
        self.playing.get() || self.clock.get().base_ms > 0
    }

    /// A song that could not be played is skipped, but a run of them means
    /// something is wrong with playback itself; stop instead of racing
    /// through the whole list.
    fn on_failed_track(self: &Rc<Self>) {
        let failures = self.failures.get() + 1;
        self.failures.set(failures);
        if failures >= 3 {
            self.failures.set(0);
            self.set_playing(false, 0);
            self.toast("Spotify won't play these songs right now; try again in a moment");
        } else {
            self.toast("This song isn't available, skipping it");
            self.next();
        }
    }

    pub(super) fn tick(&self) {
        let Some(track) = self.shown_track() else { return };
        let position = self.clock.get().now_ms().min(track.duration_ms);
        if let Some(player) = self.mpris.borrow().as_ref() {
            player.set_position(mpris_server::Time::from_millis(i64::from(position)));
        }
        self.lyrics_tick(position);
        if self.pending_seek.borrow().is_some() || !self.playing.get() {
            return;
        }
        self.bar.seek.set_value(f64::from(position));
        self.bar.position.set_label(&format_time(position));
        self.show_mini_position(position);
        // Every so often, in case Tune does not get to quit cleanly.
        let ticks = self.ticks.get() + 1;
        self.ticks.set(ticks % SAVE_EVERY_TICKS);
        if ticks >= SAVE_EVERY_TICKS && self.remote.borrow().is_none() {
            self.save_state();
        }
    }

    /// "off", minutes as a number, or "end" (when the current song ends).
    pub(super) fn set_sleep_timer(self: &Rc<Self>, choice: &str) {
        if let Some(source) = self.sleep_timer.take() {
            source.remove();
        }
        self.sleep_at_end.set(choice == "end");
        if let Some(action) = self.application.lookup_action("sleep").and_downcast::<gtk::gio::SimpleAction>() {
            action.set_state(&choice.to_variant());
        }
        match choice {
            "off" => {}
            "end" => self.toast("Music stops when this song ends"),
            minutes => {
                let Ok(minutes) = minutes.parse::<u32>() else { return };
                let app = Rc::downgrade(self);
                let source = glib::timeout_add_seconds_local_once(minutes * 60, move || {
                    let Some(app) = app.upgrade() else { return };
                    app.sleep_timer.replace(None);
                    if app.playing.get() {
                        app.toggle_play();
                    }
                    app.set_sleep_timer("off");
                    app.toast("Sleep timer: music paused");
                });
                self.sleep_timer.replace(Some(source));
                self.toast(&format!("Music pauses in {minutes} minutes"));
            }
        }
    }

    // Now Playing

    pub(super) fn toggle_now_playing(self: &Rc<Self>) {
        if self.shown_track().is_none() {
            return;
        }
        let page = self.now_playing_view().page.clone();
        self.toggle_page(&page);
        self.update_now_playing();
    }

    /// Shows `page` over the current one, or goes back from it.
    pub(super) fn toggle_page(&self, page: &adw::NavigationPage) {
        if self.nav.visible_page().as_ref() == Some(page) {
            self.nav.pop();
        } else if self.nav.navigation_stack().iter::<adw::NavigationPage>().flatten().any(|p| &p == page) {
            self.nav.pop_to_page(page);
        } else {
            self.nav.push(page);
        }
    }

    fn now_playing_view(self: &Rc<Self>) -> std::cell::Ref<'_, NowPlaying> {
        if self.now_playing.borrow().is_none() {
            let backdrop = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .can_shrink(true)
                .css_classes(["now-playing-backdrop"])
                .build();
            let art = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .can_shrink(true)
                .overflow(gtk::Overflow::Hidden)
                .css_classes(["album-art", "huge"])
                .build();
            let frame = gtk::AspectFrame::builder().ratio(1.0).obey_child(false).child(&art).build();
            let label = |classes: &[&str], lines: i32| {
                gtk::Label::builder()
                    .justify(gtk::Justification::Center)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .lines(lines)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(classes.iter().map(|c| c.to_string()).collect::<Vec<_>>())
                    .build()
            };
            let title = label(&["now-playing-title"], 2);
            let artist = label(&["now-playing-artist", "links"], 2);
            let album = label(&["dim-label", "links"], 1);
            for label in [&artist, &album] {
                let app = Rc::downgrade(self);
                label.connect_activate_link(move |_, link| app.upgrade().map_or(glib::Propagation::Proceed, |app| app.open_link(link)));
            }
            frame.set_vexpand(true);
            frame.set_valign(gtk::Align::Fill);
            let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).margin_top(22).build();
            text.append(&title);
            text.append(&artist);
            text.append(&album);
            let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
            column.append(&adw::Clamp::builder().maximum_size(420).child(&frame).vexpand(true).build());
            column.append(&adw::Clamp::builder().maximum_size(560).child(&text).build());
            let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
            column.set_vexpand(true);
            column.set_margin_start(24);
            column.set_margin_end(24);
            column.set_margin_top(64);
            column.set_margin_bottom(16);
            body.append(&column);
            body.append(&super::player_clearance());

            let shade = gtk::Box::builder().css_classes(["now-playing-shade"]).build();
            let overlay = gtk::Overlay::builder().child(&backdrop).css_classes(["now-playing-view"]).build();
            overlay.add_overlay(&shade);
            overlay.add_overlay(&body);
            let header = adw::HeaderBar::builder().title_widget(&adw::WindowTitle::new("Now Playing", "")).build();
            let view = adw::ToolbarView::builder().content(&overlay).extend_content_to_top_edge(true).build();
            view.add_top_bar(&header);
            let page = adw::NavigationPage::builder().child(&view).title("Now Playing").tag("now-playing").build();
            self.now_playing.replace(Some(NowPlaying { page, backdrop, art, title, artist, album }));
        }
        std::cell::Ref::map(self.now_playing.borrow(), |view| view.as_ref().expect("built"))
    }

    fn update_now_playing(self: &Rc<Self>) {
        let Some(track) = self.shown_track() else { return };
        let Some(url) = track.image_large.clone().or(track.image_small.clone()) else { return };
        {
            let Some(view) = &*self.now_playing.borrow() else { return };
            view.title.set_label(&track.name);
            view.artist.set_markup(&super::artist_links(&track));
            view.album.set_markup(&super::album_link(&track, &track.album));
        }
        let app = self.clone();
        let uri = track.uri.clone();
        glib::spawn_future_local(async move {
            let Some(texture) = app.texture(url).await else { return };
            if app.shown_track().is_none_or(|t| t.uri != uri) {
                return;
            }
            if let Some(view) = &*app.now_playing.borrow() {
                view.art.set_paintable(Some(&texture));
                view.backdrop.set_paintable(Some(&texture));
            }
        });
    }

    // The queue page

    pub(super) fn toggle_queue_page(self: &Rc<Self>) {
        if self.queue_page.borrow().is_none() {
            let content = adw::Bin::new();
            let (page, _) = super::content_page("Queue", &content);
            page.set_tag(Some("queue"));
            self.queue_page.replace(Some(QueuePage { page, content }));
        }
        let page = self.queue_page.borrow().as_ref().map(|q| q.page.clone()).expect("built");
        self.toggle_page(&page);
        self.refresh_queue_page();
    }

    /// Redraws the Queue page, when it is showing.
    pub(super) fn refresh_queue_page(self: &Rc<Self>) {
        let Some(content) = self.queue_page.borrow().as_ref().filter(|q| self.nav.visible_page().as_ref() == Some(&q.page)).map(|q| q.content.clone())
        else {
            return;
        };
        if self.remote.borrow().is_some() {
            content.set_child(Some(
                &adw::StatusPage::builder()
                    .icon_name("network-wireless-symbolic")
                    .title("Playing from Another App")
                    .description("The queue belongs to the Spotify app that is playing through Tune.")
                    .build(),
            ));
            return;
        }
        let (current, upcoming, next, source) = {
            let queue = self.queue.borrow();
            (queue.current().cloned(), queue.upcoming.iter().cloned().collect::<Vec<_>>(), queue.next_in_list(QUEUE_PREVIEW), queue.source.clone())
        };
        let Some(current) = current else {
            content.set_child(Some(
                &adw::StatusPage::builder().icon_name("view-list-symbolic").title("Nothing in the Queue").description("Play a song, or add one with its menu.").build(),
            ));
            return;
        };
        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        let heading = |text: &str| gtk::Label::builder().label(text).xalign(0.0).hexpand(true).css_classes(["queue-heading", "title-4"]).build();
        column.append(&heading("Now Playing"));
        column.append(&self.song_box(std::slice::from_ref(&current), |_| {}));

        if !upcoming.is_empty() {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            line.append(&heading("Next in Queue"));
            let clear = gtk::Button::builder().label("Clear").valign(gtk::Align::End).css_classes(["flat", "pill"]).build();
            clear.connect_clicked({
                let app = Rc::downgrade(self);
                move |_| {
                    if let Some(app) = app.upgrade() {
                        app.queue.borrow_mut().upcoming.clear();
                        app.save_state();
                        app.refresh_queue_page();
                    }
                }
            });
            line.append(&clear);
            column.append(&line);
            let app = Rc::downgrade(self);
            column.append(&self.song_box(&upcoming, move |index| {
                let Some(app) = app.upgrade() else { return };
                let track = app.queue.borrow_mut().upcoming.remove(index);
                if let Some(track) = track {
                    app.queue.borrow_mut().interlude = Some(track);
                    app.play_current();
                }
            }));
        }
        if !next.is_empty() {
            column.append(&heading(&if source.is_empty() { "Next Up".to_owned() } else { format!("Next From: {source}") }));
            let app = Rc::downgrade(self);
            column.append(&self.song_box(&next, move |index| {
                let Some(app) = app.upgrade() else { return };
                {
                    let mut queue = app.queue.borrow_mut();
                    queue.interlude = None;
                    queue.position += index + 1;
                }
                app.play_current();
            }));
        }
        let scrolled = super::scrolled_clamp(&column, 900);
        content.set_child(Some(&scrolled));
    }

    // Media controls

    pub(super) async fn setup_mpris(self: Rc<Self>) {
        let Some(player) = mpris::new().await else { return };
        let weak = Rc::downgrade(&self);
        let with = move |f: fn(&Rc<App>)| {
            let weak = weak.clone();
            move |_: &mpris_server::Player| {
                if let Some(app) = weak.upgrade() {
                    f(&app);
                }
            }
        };
        player.connect_play_pause(with(|app| app.toggle_play()));
        player.connect_play(with(|app| {
            if !app.playing.get() {
                app.toggle_play();
            }
        }));
        player.connect_pause(with(|app| {
            if app.playing.get() {
                app.toggle_play();
            }
        }));
        player.connect_next(with(|app| app.next()));
        player.connect_previous(with(|app| app.previous()));
        player.connect_raise(with(|app| app.window.present()));
        player.connect_set_shuffle({
            let weak = Rc::downgrade(&self);
            move |_, shuffle| {
                if let Some(app) = weak.upgrade() {
                    app.bar.shuffle.set_active(shuffle);
                }
            }
        });
        player.connect_set_loop_status({
            let weak = Rc::downgrade(&self);
            move |_, status| {
                let Some(app) = weak.upgrade() else { return };
                let wanted = mpris::repeat_from(status);
                // Repeat cycles; step until it lands on what was asked for.
                for _ in 0..3 {
                    if app.settings.borrow().repeat == wanted {
                        break;
                    }
                    app.cycle_repeat();
                }
            }
        });
        player.connect_set_volume({
            let weak = Rc::downgrade(&self);
            move |_, volume| {
                if let Some(app) = weak.upgrade() {
                    app.bar.volume.set_value(volume.clamp(0.0, 1.0));
                }
            }
        });
        player.connect_seek({
            let weak = Rc::downgrade(&self);
            move |_, offset| {
                if let Some(app) = weak.upgrade() {
                    app.seek_by(offset.as_millis());
                }
            }
        });
        player.connect_set_position({
            let weak = Rc::downgrade(&self);
            move |_, _, position| {
                if let Some(app) = weak.upgrade() {
                    app.seek_to(position.as_millis().max(0) as u32);
                }
            }
        });
        let repeat = self.settings.borrow().repeat;
        mpris::set_repeat(&player, repeat).await;
        let _ = player.set_shuffle(self.bar.shuffle.is_active()).await;
        let volume = if self.bar.mute.is_active() { 0.0 } else { self.bar.volume.value() };
        let _ = player.set_volume(volume).await;
        if let Some(track) = self.shown_track() {
            mpris::set_track(&player, &track).await;
        }
        self.mpris.replace(Some(Rc::new(player)));
    }
}
