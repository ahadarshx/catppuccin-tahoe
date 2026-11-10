//! Song rows, the lists they sit in, and the menu on each song.
//!
//! Long lists are `GtkListView`s: only the rows on screen exist, and they
//! are reused while scrolling, so a playlist of thousands opens at once.
//! A list can also hold other widgets (a page header, shelves, the space
//! under the player) as entries of their own.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::playlists::PlaylistEditor;
use super::{App, art_image, format_time};
use crate::api::Track;
use crate::tokio;

const ART_SIZE: i32 = 44;

#[derive(Clone, Copy, Default, PartialEq)]
pub struct RowOptions {
    /// Track numbers instead of covers, for albums.
    pub numbered: bool,
    /// An album column, on wide windows.
    pub album: bool,
    /// A "date added" column, on wide windows.
    pub added: bool,
}

pub struct SongRow {
    pub root: gtk::Box,
    art: gtk::Image,
    play_icon: gtk::Image,
    cover: gtk::Overlay,
    number: gtk::Label,
    title: gtk::Label,
    artists: gtk::Label,
    album: gtk::Label,
    added: gtk::Label,
    duration: gtk::Label,
    more: gtk::MenuButton,
    track: RefCell<Option<Track>>,
    options: Cell<RowOptions>,
    /// Which song of its list this row shows.
    pub index: Cell<Option<usize>>,
    /// Set when the row is in a playlist the user may change.
    editor: RefCell<Option<Rc<PlaylistEditor>>>,
}

impl SongRow {
    pub fn uri(&self) -> Option<String> {
        self.track.borrow().as_ref().map(|t| t.uri.clone())
    }

    pub fn set_playing(&self, playing: bool) {
        if playing {
            self.root.add_css_class("now-playing");
            self.play_icon.set_icon_name(Some("audio-volume-high-symbolic"));
        } else {
            self.root.remove_css_class("now-playing");
            self.play_icon.set_icon_name(Some("media-playback-start-symbolic"));
        }
    }

    pub fn set_wide(&self, wide: bool) {
        let options = self.options.get();
        self.album.set_visible(wide && options.album);
        self.added.set_visible(wide && options.added);
    }
}

/// What a list entry is: one of the list's songs, or some other widget.
pub enum Entry {
    Song(usize),
    Widget(gtk::Widget),
}

pub fn entry(item: &glib::Object) -> Option<std::cell::Ref<'_, Entry>> {
    item.downcast_ref::<glib::BoxedAnyObject>().map(|boxed| boxed.borrow::<Entry>())
}

/// A scrolling list of songs and other entries.
pub struct SongList {
    scrolled: gtk::ScrolledWindow,
    view: gtk::ListView,
    store: gio::ListStore,
    pub tracks: Rc<RefCell<Vec<Track>>>,
    /// Entries after the songs (shelves, the space under the player).
    trailing: Cell<u32>,
    /// Set for a playlist the user may change.
    pub editor: Option<Rc<PlaylistEditor>>,
}

impl SongList {
    /// The list, ready to show. The view gets its model only now: a list
    /// view stays anchored to its first entry, so songs inserted before
    /// the bottom spacer would leave it scrolled to the end.
    pub fn ready(&self) -> gtk::ScrolledWindow {
        if self.view.model().is_none() {
            self.view.set_model(Some(&gtk::NoSelection::new(Some(self.store.clone()))));
        }
        self.scrolled.clone()
    }

    /// A widget before the songs, such as the page header.
    pub fn push_leading(&self, widget: &impl IsA<gtk::Widget>) {
        let at = self.store.n_items() - self.trailing.get();
        self.store.insert(at, &glib::BoxedAnyObject::new(Entry::Widget(widget.clone().upcast())));
    }

    /// A widget after the songs; the space under the player stays last.
    pub fn push_trailing(&self, widget: &impl IsA<gtk::Widget>) {
        let at = self.store.n_items().saturating_sub(1);
        self.store.insert(at, &glib::BoxedAnyObject::new(Entry::Widget(widget.clone().upcast())));
        self.trailing.set(self.trailing.get() + 1);
    }

    pub fn append(&self, new: Vec<Track>) {
        let start = self.tracks.borrow().len();
        let entries: Vec<glib::Object> =
            (start..start + new.len()).map(|i| glib::BoxedAnyObject::new(Entry::Song(i)).upcast()).collect();
        self.tracks.borrow_mut().extend(new);
        let at = self.store.n_items() - self.trailing.get();
        self.store.splice(at, 0, &entries);
    }
}

/// Placeholder rows while a list loads.
pub fn skeleton_rows(count: usize) -> gtk::Box {
    let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).margin_top(8).build();
    for i in 0..count {
        let row = gtk::Box::builder().spacing(12).margin_start(10).margin_end(10).margin_top(6).margin_bottom(6).build();
        row.append(&gtk::Box::builder().width_request(ART_SIZE).height_request(ART_SIZE).css_classes(["skeleton"]).build());
        let lines = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).valign(gtk::Align::Center).build();
        // Uneven widths, like real titles.
        let width = [220, 160, 260, 190, 140, 240][i % 6];
        lines.append(&gtk::Box::builder().width_request(width).height_request(12).halign(gtk::Align::Start).css_classes(["skeleton", "line"]).build());
        lines.append(&gtk::Box::builder().width_request(width / 2).height_request(10).halign(gtk::Align::Start).css_classes(["skeleton", "line"]).build());
        row.append(&lines);
        column.append(&row);
    }
    column
}

impl App {
    pub(super) fn song_row(self: &Rc<Self>, options: RowOptions) -> Rc<SongRow> {
        let art = art_image(ART_SIZE, &[]);
        let play_icon = gtk::Image::builder().icon_name("media-playback-start-symbolic").css_classes(["row-play"]).build();
        let cover = gtk::Overlay::builder().child(&art).valign(gtk::Align::Center).build();
        cover.add_overlay(&play_icon);
        let number = gtk::Label::builder().xalign(0.5).css_classes(["dim-label", "track-number"]).build();
        let title = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["title"]).build();
        let artists = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .use_markup(true)
            .css_classes(["dim-label", "caption", "links"])
            .build();
        let labels = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).valign(gtk::Align::Center).hexpand(true).build();
        labels.append(&title);
        labels.append(&artists);
        let album = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .use_markup(true)
            .width_chars(24)
            .max_width_chars(24)
            .css_classes(["dim-label", "links"])
            .visible(false)
            .build();
        let added = gtk::Label::builder().xalign(0.0).width_chars(12).max_width_chars(12).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["dim-label"]).visible(false).build();
        let duration = gtk::Label::builder().css_classes(["dim-label", "track-duration"]).build();
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("More")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular", "more"])
            .build();
        let root = gtk::Box::builder().spacing(12).css_classes(["song-row"]).build();
        root.append(&cover);
        root.append(&number);
        root.append(&labels);
        root.append(&album);
        root.append(&added);
        root.append(&duration);
        root.append(&more);

        let row = Rc::new(SongRow {
            root,
            art,
            play_icon,
            cover,
            number,
            title,
            artists,
            album,
            added,
            duration,
            more,
            track: RefCell::new(None),
            options: Cell::new(options),
            index: Cell::new(None),
            editor: RefCell::new(None),
        });
        row.cover.set_visible(!options.numbered);
        row.number.set_visible(options.numbered);
        row.set_wide(self.wide.get());

        for label in [&row.artists, &row.album] {
            let app = Rc::downgrade(self);
            label.connect_activate_link(move |_, link| app.upgrade().map_or(glib::Propagation::Proceed, |app| app.open_link(link)));
        }
        // The menu is made fresh each time, for the song shown now.
        row.more.set_create_popup_func({
            let app = Rc::downgrade(self);
            let row = Rc::downgrade(&row);
            move |button| {
                let (Some(app), Some(row)) = (app.upgrade(), row.upgrade()) else { return };
                let Some(track) = row.track.borrow().clone() else { return };
                if let Some(popover) = button.popover() {
                    popover.set_pointing_to(None);
                }
                let editing = row.editor.borrow().clone().zip(row.index.get());
                app.attach_song_menu(button.upcast_ref(), button, &track, editing);
            }
        });
        // Right click opens the same menu where the pointer is.
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        click.connect_pressed({
            let row = Rc::downgrade(&row);
            move |gesture, _, x, y| {
                let Some(row) = row.upgrade() else { return };
                gesture.set_state(gtk::EventSequenceState::Claimed);
                row.more.popup();
                if let Some(popover) = row.more.popover()
                    && let Some(point) = row.root.compute_point(&row.more, &gtk::graphene::Point::new(x as f32, y as f32))
                {
                    popover.set_pointing_to(Some(&gdk::Rectangle::new(point.x() as i32, point.y() as i32, 1, 1)));
                }
            }
        });
        row.root.add_controller(click);

        self.rows.borrow_mut().push(Rc::downgrade(&row));
        row
    }

    pub(super) fn bind_song(self: &Rc<Self>, row: &Rc<SongRow>, track: &Track) {
        row.track.replace(Some(track.clone()));
        row.title.set_label(&track.name);
        row.title.set_tooltip_text(Some(&track.name));
        row.artists.set_markup(&super::artist_links(track));
        row.album.set_markup(&super::album_link(track, &track.album));
        row.added.set_label(&track.added_at.as_deref().map(format_added).unwrap_or_default());
        row.duration.set_label(&format_time(track.duration_ms));
        row.number.set_label(&if track.track_number > 0 { track.track_number.to_string() } else { String::new() });
        let playing = self.shown_track().is_some_and(|t| t.uri == track.uri);
        row.set_playing(playing);

        row.art.set_paintable(gdk::Paintable::NONE);
        row.art.set_icon_name(Some("audio-x-generic-symbolic"));
        row.art.set_pixel_size(16);
        row.art.add_css_class("placeholder");
        if !row.options.get().numbered
            && let Some(url) = track.image_small.clone()
        {
            let weak = Rc::downgrade(row);
            let uri = track.uri.clone();
            self.load_image_if(&row.art, url, ART_SIZE, move || weak.upgrade().and_then(|row| row.uri()).as_deref() == Some(uri.as_str()));
        }
    }

    /// A list of songs; activating one plays the list from there.
    /// `source` names the list in the queue ("Liked Songs").
    /// `playlist` is the ID of a playlist the user may change, if this is one.
    pub(super) fn song_list(self: &Rc<Self>, options: RowOptions, source: &str, playlist: Option<String>) -> SongList {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let tracks: Rc<RefCell<Vec<Track>>> = Rc::default();
        let editor = playlist.map(|id| {
            Rc::new(PlaylistEditor { id, tracks: tracks.clone(), store: store.clone(), ready: Cell::new(false) })
        });
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup({
            let app = Rc::downgrade(self);
            let tracks = tracks.clone();
            let editor = editor.clone();
            move |_, item| {
                let Some(app) = app.upgrade() else { return };
                let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
                let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
                let row = app.song_row(options);
                if let Some(editor) = &editor {
                    row.editor.replace(Some(editor.clone()));
                    app.make_reorderable(&row, editor);
                }
                container.append(&row.root);
                item.set_child(Some(&container));
                let app = Rc::downgrade(&app);
                let tracks = tracks.clone();
                item.connect_item_notify(move |item| {
                    let Some(app) = app.upgrade() else { return };
                    // Whatever widget this row held for another entry goes.
                    while let Some(child) = row.root.next_sibling() {
                        container.remove(&child);
                    }
                    let Some(object) = item.item() else { return };
                    let Some(entry) = entry(&object) else { return };
                    match &*entry {
                        Entry::Song(index) => {
                            item.set_activatable(true);
                            item.set_focusable(true);
                            row.root.set_visible(true);
                            row.index.set(Some(*index));
                            if let Some(track) = tracks.borrow().get(*index) {
                                app.bind_song(&row, track);
                            }
                        }
                        Entry::Widget(widget) => {
                            item.set_activatable(false);
                            item.set_focusable(false);
                            row.root.set_visible(false);
                            row.index.set(None);
                            row.track.replace(None);
                            if let Some(parent) = widget.parent().and_downcast::<gtk::Box>() {
                                parent.remove(widget);
                            }
                            container.append(widget);
                        }
                    }
                });
            }
        });
        let view = gtk::ListView::builder()
            .factory(&factory)
            .single_click_activate(true)
            .css_classes(["songs"])
            .margin_start(12)
            .margin_end(12)
            .build();
        let source = source.to_owned();
        view.connect_activate({
            let app = Rc::downgrade(self);
            let tracks = tracks.clone();
            move |view, position| {
                let Some(app) = app.upgrade() else { return };
                let Some(object) = view.model().and_then(|model| model.item(position)) else { return };
                if let Some(Entry::Song(index)) = entry(&object).as_deref() {
                    app.play_list(tracks.borrow().clone(), *index, &source);
                }
            }
        });
        let clamp = adw::ClampScrollable::builder().maximum_size(900).child(&view).build();
        let scrolled = gtk::ScrolledWindow::builder().child(&clamp).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
        store.append(&glib::BoxedAnyObject::new(Entry::Widget(super::player_clearance().upcast())));
        SongList { scrolled, view, store, tracks, trailing: Cell::new(1), editor }
    }

    /// A short list of songs in a plain box, for the Queue page.
    pub(super) fn song_box(self: &Rc<Self>, tracks: &[Track], on_activate: impl Fn(usize) + 'static) -> gtk::ListBox {
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["songs"]).build();
        for track in tracks {
            let row = self.song_row(RowOptions::default());
            self.bind_song(&row, track);
            let holder = gtk::ListBoxRow::builder().child(&row.root).activatable(true).build();
            // The row lives as long as its widget.
            holder.connect_destroy(move |_| {
                let _ = &row;
            });
            list.append(&holder);
        }
        list.connect_row_activated(move |_, row| on_activate(row.index() as usize));
        list
    }

    /// Highlights the playing song wherever it is listed; forgets rows
    /// that are gone.
    pub(super) fn mark_now_playing(&self, uri: Option<&str>) {
        let mut rows = self.rows.borrow_mut();
        rows.retain(|row| row.strong_count() > 0);
        for row in rows.iter().filter_map(std::rc::Weak::upgrade) {
            row.set_playing(uri.is_some() && row.uri().as_deref() == uri);
        }
    }

    pub(super) fn set_wide(&self, wide: bool) {
        self.wide.set(wide);
        for row in self.rows.borrow().iter().filter_map(std::rc::Weak::upgrade) {
            row.set_wide(wide);
        }
    }

    /// Gives `button` a menu for `track`: queueing, going to its album or
    /// artist, saving, adding to a playlist, and copying its link.
    pub(super) fn attach_song_menu(
        self: &Rc<Self>,
        holder: &gtk::Widget,
        button: &gtk::MenuButton,
        track: &Track,
        editing: Option<(Rc<PlaylistEditor>, usize)>,
    ) {
        let group = gio::SimpleActionGroup::new();
        let add = |name: &str, parameter: Option<&glib::VariantTy>, f: Box<dyn Fn(Option<&glib::Variant>)>| {
            let action = gio::SimpleAction::new(name, parameter);
            action.connect_activate(move |_, value| f(value));
            group.add_action(&action);
        };
        let weak = Rc::downgrade(self);
        let with = |f: fn(&Rc<App>, &Track, Option<&glib::Variant>)| {
            let weak = weak.clone();
            let track = track.clone();
            Box::new(move |value: Option<&glib::Variant>| {
                if let Some(app) = weak.upgrade() {
                    f(&app, &track, value);
                }
            }) as Box<dyn Fn(Option<&glib::Variant>)>
        };
        add("play-next", None, with(|app, track, _| app.queue_song(track.clone(), true)));
        add("add-queue", None, with(|app, track, _| app.queue_song(track.clone(), false)));
        add("go-album", None, with(|app, track, _| {
            if let Some(id) = &track.album_id {
                app.open_album(id.clone());
            }
        }));
        add("go-artist", Some(glib::VariantTy::STRING), with(|app, _, value| {
            if let Some(id) = value.and_then(|v| v.str()) {
                app.open_artist(id.to_owned());
            }
        }));
        add("add-playlist", Some(glib::VariantTy::STRING), with(|app, track, value| {
            if let Some(id) = value.and_then(|v| v.str()) {
                app.add_to_playlist(id.to_owned(), track);
            }
        }));
        add("new-playlist", None, with(|app, track, _| app.ask_new_playlist(Some(track.clone()))));
        if let Some((editor, index)) = &editing {
            let index = *index;
            let last = editor.tracks.borrow().len().saturating_sub(1);
            for (name, to) in [("move-top", 0), ("move-bottom", last)] {
                let (weak, editor) = (weak.clone(), editor.clone());
                add(name, None, Box::new(move |_| {
                    if let Some(app) = weak.upgrade() {
                        app.move_song(editor.clone(), index, to);
                    }
                }));
            }
            let (weak, editor, track) = (weak.clone(), editor.clone(), track.clone());
            add("remove-playlist", None, Box::new(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.remove_song(editor.clone(), track.clone());
                }
            }));
        }
        add("copy-link", None, with(|app, track, _| {
            let id = track.uri.rsplit(':').next().unwrap_or_default();
            app.window.clipboard().set_text(&format!("https://open.spotify.com/track/{id}"));
            app.toast("Link copied");
        }));
        // Saved or not is only known after asking Spotify; until then the
        // item waits, greyed out.
        let liked = gio::SimpleAction::new_stateful("liked", None, &false.to_variant());
        liked.set_enabled(false);
        liked.connect_change_state({
            let weak = weak.clone();
            let track = track.clone();
            move |action, state| {
                let (Some(app), Some(saved)) = (weak.upgrade(), state.and_then(bool::from_variant)) else { return };
                action.set_state(&saved.to_variant());
                app.set_song_saved(track.clone(), saved);
            }
        });
        group.add_action(&liked);
        glib::spawn_future_local({
            let backend = self.backend.clone();
            let uri = track.uri.clone();
            let liked = liked.clone();
            async move {
                if let Ok(saved) = tokio(async move { backend.is_saved(uri).await }).await {
                    liked.set_state(&saved.to_variant());
                    liked.set_enabled(true);
                }
            }
        });

        let menu = gio::Menu::new();
        let section = gio::Menu::new();
        section.append(Some("Play Next"), Some("song.play-next"));
        section.append(Some("Add to Queue"), Some("song.add-queue"));
        menu.append_section(None, &section);

        let section = gio::Menu::new();
        if track.album_id.is_some() {
            section.append(Some("Go to Album"), Some("song.go-album"));
        }
        let artist_item = |label: &str, id: &str| gio::MenuItem::new(Some(label), Some(&format!("song.go-artist::{id}")));
        match track.artist_refs.as_slice() {
            [] => {}
            [artist] => section.append_item(&artist_item("Go to Artist", &artist.id)),
            artists => {
                let submenu = gio::Menu::new();
                for artist in artists {
                    submenu.append_item(&artist_item(&artist.name, &artist.id));
                }
                section.append_submenu(Some("Go to Artist"), &submenu);
            }
        }
        menu.append_section(None, &section);

        let section = gio::Menu::new();
        section.append(Some("In Liked Songs"), Some("song.liked"));
        {
            let submenu = gio::Menu::new();
            let create = gio::Menu::new();
            create.append(Some("New Playlist…"), Some("song.new-playlist"));
            submenu.append_section(None, &create);
            let existing = gio::Menu::new();
            for playlist in self.editable_playlists() {
                existing.append_item(&gio::MenuItem::new(Some(&playlist.name), Some(&format!("song.add-playlist::{}", playlist.id))));
            }
            submenu.append_section(None, &existing);
            section.append_submenu(Some("Add to Playlist"), &submenu);
        }
        menu.append_section(None, &section);

        if let Some((editor, index)) = &editing
            && editor.ready.get()
        {
            let section = gio::Menu::new();
            if *index > 0 {
                section.append(Some("Move to Top"), Some("song.move-top"));
            }
            if *index + 1 < editor.tracks.borrow().len() {
                section.append(Some("Move to Bottom"), Some("song.move-bottom"));
            }
            section.append(Some("Remove from This Playlist"), Some("song.remove-playlist"));
            menu.append_section(None, &section);
        }

        let section = gio::Menu::new();
        section.append(Some("Copy Song Link"), Some("song.copy-link"));
        menu.append_section(None, &section);

        holder.insert_action_group("song", Some(&group));
        button.set_menu_model(Some(&menu));
    }

    pub(super) fn set_song_saved(self: &Rc<Self>, track: Track, saved: bool) {
        let app = self.clone();
        glib::spawn_future_local(async move {
            let backend = app.backend.clone();
            let uri = track.uri.clone();
            match tokio(async move { backend.set_saved(uri, saved).await }).await {
                Ok(()) => {
                    if app.shown_track().is_some_and(|t| t.uri == track.uri) {
                        app.show_liked(Some(saved));
                    }
                    app.toast(if saved { "Added to Liked Songs" } else { "Removed from Liked Songs" });
                }
                Err(e) => app.toast(&e),
            }
        });
    }

    fn add_to_playlist(self: &Rc<Self>, playlist: String, track: &Track) {
        let name = self.playlists.borrow().iter().find(|p| p.id == playlist).map(|p| p.name.clone()).unwrap_or_default();
        let app = self.clone();
        let uri = track.uri.clone();
        glib::spawn_future_local(async move {
            let backend = app.backend.clone();
            match tokio(async move { backend.add_to_playlist(playlist, uri).await }).await {
                Ok(()) => app.toast(&format!("Added to {name}")),
                Err(e) => app.toast(&e),
            }
        });
    }
}

/// "Today", "3 days ago", or the date, for when a song was added.
fn format_added(timestamp: &str) -> String {
    let Ok(added) = glib::DateTime::from_iso8601(timestamp, None) else { return String::new() };
    let Ok(now) = glib::DateTime::now_utc() else { return String::new() };
    let days = now.difference(&added).as_days();
    match days {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=29 => format!("{days} days ago"),
        _ => added.to_local().and_then(|d| d.format("%b %-d, %Y")).map(String::from).unwrap_or_default(),
    }
}
