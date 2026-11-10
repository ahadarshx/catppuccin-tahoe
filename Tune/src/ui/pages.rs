//! The content side: Home, Search, and pages of songs (Liked Songs,
//! playlists, albums, artists), each with a header to play from.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::songs::{RowOptions, SongList, skeleton_rows};
use super::{App, SidebarItem, art_image, content_page, count_label, format_length, scrolled_clamp};
use crate::api::{Album, Artist, Header, Playlist, SearchResults, Track};
use crate::backend::TrackSource;
use crate::tokio;

const CARD_ART_SIZE: i32 = 148;
const HEADER_ART_SIZE: i32 = 196;
const SHELF_ITEMS: usize = 12;
/// Upper bound on songs listed for one playlist; lists are cheap now, but
/// Spotify pages are not.
const MAX_LIST_TRACKS: usize = 10_000;

pub fn search_placeholder() -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("system-search-symbolic")
        .title("Search Spotify")
        .description("Find songs, artists, albums and playlists")
        .vexpand(true)
        .build()
}

fn greeting() -> &'static str {
    match glib::DateTime::now_local().map(|now| now.hour()).unwrap_or(12) {
        5..=11 => "Good morning",
        12..=16 => "Good afternoon",
        _ => "Good evening",
    }
}

/// A message in place of a page, with a button to try again.
fn error_page(title: &str, message: &str, retry: impl Fn() + 'static) -> adw::StatusPage {
    let button = gtk::Button::builder().label("Try Again").halign(gtk::Align::Center).css_classes(["pill"]).build();
    button.connect_clicked(move |_| retry());
    adw::StatusPage::builder().icon_name("dialog-warning-symbolic").title(title).description(message).child(&button).vexpand(true).build()
}

/// Loading placeholders, then whatever the page loads.
fn loading_stack() -> gtk::Stack {
    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
    stack.add_named(&scrolled_clamp(&skeleton_rows(10), 900), Some("loading"));
    stack
}

fn show_in(stack: &gtk::Stack, widget: &impl IsA<gtk::Widget>) {
    if let Some(old) = stack.child_by_name("done") {
        stack.remove(&old);
    }
    stack.add_named(widget, Some("done"));
    stack.set_visible_child_name("done");
}

fn section_title(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).css_classes(["section-title"]).margin_start(6).margin_bottom(6).build()
}

/// What a page header shows, and its count to fill in as songs arrive.
struct PageHeader {
    buttons: gtk::Box,
    title: gtk::Label,
    description: gtk::Label,
    root: gtk::Box,
    count: gtk::Label,
    byline: String,
}

impl PageHeader {
    fn set_count(&self, tracks: &[Track], total: Option<usize>) {
        let length: u64 = tracks.iter().map(|t| u64::from(t.duration_ms)).sum();
        let mut parts: Vec<String> = Vec::new();
        if !self.byline.is_empty() {
            parts.push(self.byline.clone());
        }
        parts.push(count_label(total.unwrap_or(tracks.len()).max(tracks.len())));
        if total.is_none_or(|total| total <= tracks.len()) && length > 0 {
            parts.push(format_length(length));
        }
        self.count.set_label(&parts.join(" • "));
    }
}

impl App {
    pub(super) fn open_sidebar_item(self: &Rc<Self>, index: i32) {
        self.split.set_show_content(true);
        let Some(item) = self.sidebar.items.borrow().get(index as usize).cloned() else { return };
        match item {
            SidebarItem::Home => {
                self.next_generation();
                let page = self.home_page();
                self.nav.replace(&[page]);
            }
            SidebarItem::Search => {
                self.next_generation();
                self.nav.replace(std::slice::from_ref(&self.search_page));
                self.search_entry.grab_focus();
            }
            SidebarItem::Liked => self.open_tracks("Liked Songs", TrackSource::Liked, false),
            SidebarItem::Playlist(playlist) => self.open_tracks(&playlist.name, TrackSource::Playlist(playlist.id), false),
        }
    }

    pub(super) fn open_album(self: &Rc<Self>, id: String) {
        self.open_tracks("Album", TrackSource::Album(id), true);
    }

    fn open_playlist(self: &Rc<Self>, playlist: &Playlist) {
        self.open_tracks(&playlist.name, TrackSource::Playlist(playlist.id.clone()), true);
    }

    /// A page of songs, as the new root (sidebar) or on top (album, artist).
    pub(super) fn open_tracks(self: &Rc<Self>, title: &str, source: TrackSource, push: bool) {
        let stack = loading_stack();
        let (page, window_title) = content_page(title, &stack);
        let generation = if push { self.generation.get() } else { self.next_generation() };
        if push {
            self.nav.push(&page);
        } else {
            self.nav.replace(std::slice::from_ref(&page));
        }
        let load: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::default();
        let loader: Rc<dyn Fn()> = Rc::new({
            let app = Rc::downgrade(self);
            let load = Rc::downgrade(&load);
            let title = title.to_owned();
            move || {
                let (Some(app), Some(load)) = (app.upgrade(), load.upgrade()) else { return };
                stack.set_visible_child_name("loading");
                let retry = load.borrow().as_ref().map(Rc::downgrade);
                glib::spawn_future_local(app.clone().fill_tracks(stack.clone(), window_title.clone(), title.clone(), source.clone(), generation, retry));
            }
        });
        load.replace(Some(loader.clone()));
        // The page keeps its loader for Try Again.
        stack_keepalive(&page, load);
        loader();
    }

    async fn fill_tracks(
        self: Rc<Self>,
        stack: gtk::Stack,
        window_title: adw::WindowTitle,
        title: String,
        source: TrackSource,
        generation: u64,
        retry: Option<std::rc::Weak<dyn Fn()>>,
    ) {
        let backend = self.backend.clone();
        let request = source.clone();
        let result = tokio(async move { backend.first_page(request).await }).await;
        // The sidebar moved on to something else meanwhile.
        if self.generation.get() != generation {
            return;
        }
        let (header, first) = match result {
            Ok(loaded) => loaded,
            Err(e) => {
                show_in(&stack, &error_page("Couldn't Load These Songs", &e, move || {
                    if let Some(retry) = retry.as_ref().and_then(std::rc::Weak::upgrade) {
                        retry();
                    }
                }));
                return;
            }
        };
        let options = match source {
            TrackSource::Album(_) => RowOptions { numbered: true, ..RowOptions::default() },
            _ => RowOptions { album: true, added: true, ..RowOptions::default() },
        };
        let name = header.as_ref().map(|h| h.title.clone()).filter(|t| !t.is_empty()).unwrap_or(title);
        window_title.set_title(&name);
        let playlist_id = match &source {
            TrackSource::Playlist(id) => Some(id.clone()),
            _ => None,
        };
        // Songs can be changed in collaborative playlists too; the details only by their owner.
        let editable = playlist_id.clone().filter(|id| self.editable_playlists().iter().any(|p| p.id == *id));
        let owned = editable.clone().filter(|id| {
            let me = self.backend.user_id();
            self.playlists.borrow().iter().any(|p| p.id == *id && p.owner_id == me)
        });
        let list = self.song_list(options, &name, editable);
        let header = match (&source, header) {
            (TrackSource::Liked, _) => self.page_header(
                &Header { kind: "Playlist".into(), title: "Liked Songs".into(), ..Header::default() },
                Some(liked_tile()),
                false,
                &list,
            ),
            (_, Some(header)) => self.page_header(&header, None, false, &list),
            (_, None) => self.page_header(&Header { kind: "Playlist".into(), title: name.clone(), ..Header::default() }, None, false, &list),
        };
        if let Some(id) = &owned {
            header.buttons.append(&self.playlist_menu(id, &header.title, &header.description, &window_title));
        }
        list.push_leading(&header.root);
        if first.tracks.is_empty() && first.next.is_none() {
            list.push_trailing(
                &adw::StatusPage::builder().icon_name("folder-music-symbolic").title("No Songs Here").build(),
            );
        }
        let total = first.total;
        list.append(first.tracks);
        header.set_count(&list.tracks.borrow(), total);
        window_title.set_subtitle(&count_label(total.unwrap_or(list.tracks.borrow().len())));
        show_in(&stack, &list.ready());

        // The first songs show right away; the rest are added as they arrive.
        let mut next = first.next;
        while let Some(url) = next.take() {
            if self.generation.get() != generation || list.tracks.borrow().len() >= MAX_LIST_TRACKS || !stack.is_mapped() && stack.root().is_none() {
                break;
            }
            let backend = self.backend.clone();
            match tokio(async move { backend.next_page(url).await }).await {
                Ok(page) => {
                    list.append(page.tracks);
                    header.set_count(&list.tracks.borrow(), total);
                    next = page.next;
                }
                Err(e) => log::warn!("loading more songs: {e}"),
            }
        }
        if let Some(editor) = &list.editor {
            editor.ready.set(true);
        }
    }

    /// The top of a song page: cover, kind, title, byline, and Play and
    /// Shuffle buttons for the list below.
    fn page_header(self: &Rc<Self>, header: &Header, tile: Option<gtk::Widget>, round: bool, list: &SongList) -> PageHeader {
        let cover: gtk::Widget = match tile {
            Some(tile) => tile,
            None => {
                let classes: &[&str] = if round { &["huge", "round"] } else { &["huge"] };
                let art = art_image(HEADER_ART_SIZE, classes);
                if let Some(url) = &header.image {
                    self.load_image(&art, url.clone(), HEADER_ART_SIZE);
                }
                art.upcast()
            }
        };
        let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).valign(gtk::Align::End).hexpand(true).build();
        text.append(&gtk::Label::builder().label(header.kind.to_uppercase()).xalign(0.0).css_classes(["kind", "dim-label"]).build());
        let title = gtk::Label::builder()
            .label(&header.title)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["header-title"])
            .build();
        if header.title.chars().count() > 22 {
            title.add_css_class("long");
        }
        text.append(&title);
        let description = gtk::Label::builder()
            .label(&header.description)
            .visible(!header.description.is_empty())
            .xalign(0.0)
            .wrap(true)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["dim-label"])
            .build();
        text.append(&description);
        // Artists link to their pages; other bylines (owners) are plain.
        let byline = if header.artist_refs.is_empty() {
            header.byline.iter().map(|part| glib::markup_escape_text(part).to_string()).collect::<Vec<_>>().join(" • ")
        } else {
            let links = header
                .artist_refs
                .iter()
                .map(|a| format!("<a href=\"artist:{}\">{}</a>", a.id, glib::markup_escape_text(&a.name)))
                .collect::<Vec<_>>()
                .join(", ");
            std::iter::once(format!("<b>{links}</b>"))
                .chain(header.byline.iter().skip(1).map(|part| glib::markup_escape_text(part).to_string()))
                .collect::<Vec<_>>()
                .join(" • ")
        };
        let count = gtk::Label::builder().xalign(0.0).wrap(true).use_markup(true).css_classes(["links"]).build();
        count.connect_activate_link({
            let app = Rc::downgrade(self);
            move |_, link| app.upgrade().map_or(glib::Propagation::Proceed, |app| app.open_link(link))
        });
        text.append(&count);

        let play = gtk::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text("Play")
            .css_classes(["circular", "suggested-action", "big-play"])
            .build();
        let shuffle = gtk::Button::builder()
            .icon_name("media-playlist-shuffle-symbolic")
            .tooltip_text("Shuffle Play")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        let source = header.title.clone();
        play.connect_clicked({
            let app = Rc::downgrade(self);
            let tracks = list.tracks.clone();
            let source = source.clone();
            move |_| {
                let Some(app) = app.upgrade() else { return };
                app.syncing.set(true);
                app.bar.shuffle.set_active(false);
                app.syncing.set(false);
                app.play_list(tracks.borrow().clone(), 0, &source);
            }
        });
        shuffle.connect_clicked({
            let app = Rc::downgrade(self);
            let tracks = list.tracks.clone();
            move |_| {
                let Some(app) = app.upgrade() else { return };
                let tracks = tracks.borrow().clone();
                if tracks.is_empty() {
                    return;
                }
                app.syncing.set(true);
                app.bar.shuffle.set_active(true);
                app.syncing.set(false);
                let start = glib::random_int_range(0, tracks.len() as i32) as usize;
                app.play_list(tracks, start, &source);
            }
        });
        let buttons = gtk::Box::builder().spacing(14).margin_top(14).build();
        buttons.append(&play);
        buttons.append(&shuffle);

        let top = gtk::Box::builder().spacing(24).build();
        top.append(&cover);
        top.append(&text);
        let root = gtk::Box::builder().orientation(gtk::Orientation::Vertical).css_classes(["page-header"]).build();
        root.append(&top);
        root.append(&buttons);
        let header = PageHeader { buttons, title, description, root, count, byline };
        header.count.set_markup(&header.byline);
        header
    }

    pub(super) fn open_artist(self: &Rc<Self>, id: String) {
        let stack = loading_stack();
        let (page, window_title) = content_page("Artist", &stack);
        self.nav.push(&page);
        let load: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::default();
        let loader: Rc<dyn Fn()> = Rc::new({
            let app = Rc::downgrade(self);
            let load = Rc::downgrade(&load);
            move || {
                let (Some(app), Some(load)) = (app.upgrade(), load.upgrade()) else { return };
                stack.set_visible_child_name("loading");
                let retry = load.borrow().as_ref().map(Rc::downgrade);
                glib::spawn_future_local(app.clone().fill_artist(stack.clone(), window_title.clone(), id.clone(), retry));
            }
        });
        load.replace(Some(loader.clone()));
        stack_keepalive(&page, load);
        loader();
    }

    async fn fill_artist(self: Rc<Self>, stack: gtk::Stack, window_title: adw::WindowTitle, id: String, retry: Option<std::rc::Weak<dyn Fn()>>) {
        let backend = self.backend.clone();
        let result = tokio(async move { backend.artist_page(id).await }).await;
        let artist = match result {
            Ok(artist) => artist,
            Err(e) => {
                show_in(&stack, &error_page("Couldn't Load This Artist", &e, move || {
                    if let Some(retry) = retry.as_ref().and_then(std::rc::Weak::upgrade) {
                        retry();
                    }
                }));
                return;
            }
        };
        window_title.set_title(&artist.artist.name);
        let list = self.song_list(RowOptions { album: true, ..RowOptions::default() }, &artist.artist.name, None);
        let header = Header {
            kind: "Artist".into(),
            title: artist.artist.name.clone(),
            image: artist.artist.image.clone(),
            ..Header::default()
        };
        let page_header = self.page_header(&header, None, true, &list);
        list.push_leading(&page_header.root);
        if !artist.top_tracks.is_empty() {
            list.push_leading(&section_title("Popular"));
        }
        list.append(artist.top_tracks);
        page_header.count.set_label(&count_label(list.tracks.borrow().len()));
        let releases = artist.albums;
        if !releases.is_empty() {
            let (section, flow) = shelf_section("Discography");
            self.fill_album_shelf(&flow, releases);
            section.set_margin_top(18);
            list.push_trailing(&section);
        }
        show_in(&stack, &list.ready());
    }

    fn home_page(self: &Rc<Self>) -> adw::NavigationPage {
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(30).build();
        content.append(&gtk::Label::builder().label(greeting()).xalign(0.0).css_classes(["greeting"]).build());
        let (page, _) = content_page("Home", &scrolled_clamp(&content, 1180));

        // Each shelf shows placeholders until its request returns, and goes
        // away if Spotify has nothing for it.
        let shelf = |title: &str| {
            let (section, flow) = shelf_section(title);
            content.append(&section);
            flow
        };
        let recent = shelf("Recently played");
        let for_you = shelf("Recommended for you");
        let top_songs = shelf("Your top songs");
        let lately = shelf("Based on your recent listening");
        let top_artists = shelf("Your top artists");
        let albums = shelf("Your albums");

        // Recently played and top songs fill their own shelves, then seed
        // Spotify's song radio for the two recommendation shelves.
        let generation = self.generation.get();
        let app = self.clone();
        glib::spawn_future_local(async move {
            let backend = app.backend.clone();
            let (recent_tracks, top_tracks) =
                tokio(async move { (backend.recently_played().await, backend.top_tracks().await) }).await;
            let recent_tracks = recent_tracks.unwrap_or_default();
            let top_tracks = top_tracks.unwrap_or_default();
            app.fill_track_shelf(&recent, recent_tracks.clone(), generation, "Recently played");
            app.fill_track_shelf(&top_songs, top_tracks.clone(), generation, "Your top songs");

            // Songs already on the page are not recommended again.
            let shown: Vec<String> = recent_tracks.iter().chain(&top_tracks).take(60).map(|t| t.uri.clone()).collect();
            let seeds = |tracks: &[Track], n: usize| tracks.iter().take(n).map(|t| t.uri.clone()).collect::<Vec<_>>();
            let mut mixes = vec![(for_you, seeds(&top_tracks, 3), "Recommended for you")];
            // With no top songs yet (a new account), recent plays seed both.
            if mixes[0].1.is_empty() {
                mixes[0].1 = seeds(&recent_tracks, 3);
            }
            mixes.push((lately, seeds(&recent_tracks, 2), "Based on your recent listening"));

            let mut seen = shown;
            for (shelf, seed_uris, name) in mixes {
                if seed_uris.is_empty() {
                    app.fill_track_shelf(&shelf, Vec::new(), generation, name);
                    continue;
                }
                let backend = app.backend.clone();
                let exclude = seen.clone();
                let tracks = match tokio(async move { backend.radio(seed_uris, exclude, SHELF_ITEMS).await }).await {
                    Ok(tracks) => tracks,
                    Err(e) => {
                        log::warn!("recommendations: {e}");
                        Vec::new()
                    }
                };
                seen.extend(tracks.iter().map(|t| t.uri.clone()));
                app.fill_track_shelf(&shelf, tracks, generation, name);
            }
        });
        let app = self.clone();
        glib::spawn_future_local(async move {
            let backend = app.backend.clone();
            let artists = tokio(async move { backend.top_artists().await }).await.unwrap_or_default();
            app.fill_artist_shelf(&top_artists, artists);
        });
        let app = self.clone();
        glib::spawn_future_local(async move {
            let backend = app.backend.clone();
            let saved = tokio(async move { backend.saved_albums().await }).await.unwrap_or_else(|e| {
                log::warn!("home: {e}");
                Vec::new()
            });
            app.fill_album_shelf(&albums, saved);
        });
        page
    }

    fn card(self: &Rc<Self>, title: &str, subtitle: &str, image: Option<&String>, round: bool, on_click: impl Fn() + 'static) -> gtk::Button {
        let art = art_image(CARD_ART_SIZE, if round { &["large", "round"] } else { &["large"] });
        if let Some(url) = image {
            self.load_image(&art, url.clone(), CARD_ART_SIZE);
        }
        let play = gtk::Image::builder()
            .icon_name("media-playback-start-symbolic")
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .css_classes(["card-play"])
            .build();
        let cover = gtk::Overlay::builder().child(&art).build();
        cover.add_overlay(&play);
        // As wide as the cover, centred in its cell: text lines up with the art.
        let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).halign(gtk::Align::Center).build();
        body.append(&cover);
        let label = |text: &str, classes: &[&str]| {
            gtk::Label::builder()
                .label(text)
                .xalign(if round { 0.5 } else { 0.0 })
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(1)
                .css_classes(classes.iter().map(|c| c.to_string()).collect::<Vec<_>>())
                .build()
        };
        body.append(&label(title, &["card-title"]));
        if !subtitle.is_empty() {
            body.append(&label(subtitle, &["dim-label", "caption"]));
        }
        let button = gtk::Button::builder().child(&body).css_classes(["flat", "shelf-item"]).tooltip_text(title).build();
        button.connect_clicked(move |_| on_click());
        button
    }

    fn fill_track_shelf(self: &Rc<Self>, flow: &gtk::FlowBox, tracks: Vec<Track>, generation: u64, source: &str) {
        flow.remove_all();
        let tracks: Rc<Vec<Track>> = Rc::new(tracks.into_iter().take(SHELF_ITEMS).collect());
        for (index, track) in tracks.iter().enumerate() {
            let app = self.clone();
            let list = tracks.clone();
            let source = source.to_owned();
            let card = self.card(&track.name, &track.artists, track.image_large.as_ref(), false, move || {
                if app.generation.get() == generation {
                    app.play_list(list.to_vec(), index, &source);
                }
            });
            // The same menu as a song row, on right click.
            let menu = gtk::MenuButton::builder().visible(false).build();
            let body = gtk::Overlay::builder().child(&card).build();
            body.add_overlay(&menu);
            let click = gtk::GestureClick::builder().button(gtk::gdk::BUTTON_SECONDARY).build();
            click.connect_pressed({
                let app = Rc::downgrade(self);
                let track = track.clone();
                let body = body.clone();
                move |gesture, _, x, y| {
                    let Some(app) = app.upgrade() else { return };
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    app.attach_song_menu(body.upcast_ref(), &menu, &track, None);
                    let popover = gtk::PopoverMenu::from_model(menu.menu_model().as_ref());
                    popover.set_parent(&body);
                    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                    popover.set_has_arrow(false);
                    popover.connect_closed(|popover| {
                        let popover = popover.clone();
                        glib::idle_add_local_once(move || popover.unparent());
                    });
                    popover.popup();
                }
            });
            body.add_controller(click);
            flow.append(&body);
        }
        show_shelf(flow);
    }

    fn fill_album_shelf(self: &Rc<Self>, flow: &gtk::FlowBox, albums: Vec<Album>) {
        flow.remove_all();
        for album in albums.into_iter().take(SHELF_ITEMS) {
            let app = self.clone();
            let id = album.id.clone();
            // "2024 • Single", or the artists for albums from many artists.
            let kind = if album.kind == "album" { album.artists.clone() } else { crate::api::capitalize(&album.kind) };
            let subtitle = if album.year.is_empty() { kind } else { format!("{} • {kind}", album.year) };
            let card = self.card(&album.name, &subtitle, album.image.as_ref(), false, move || app.open_album(id.clone()));
            flow.append(&card);
        }
        show_shelf(flow);
    }

    fn fill_artist_shelf(self: &Rc<Self>, flow: &gtk::FlowBox, artists: Vec<Artist>) {
        flow.remove_all();
        for artist in artists.into_iter().take(SHELF_ITEMS) {
            let app = self.clone();
            let id = artist.id.clone();
            let card = self.card(&artist.name, "Artist", artist.image.as_ref(), true, move || app.open_artist(id.clone()));
            flow.append(&card);
        }
        show_shelf(flow);
    }

    fn fill_playlist_shelf(self: &Rc<Self>, flow: &gtk::FlowBox, playlists: Vec<Playlist>) {
        flow.remove_all();
        for playlist in playlists.into_iter().take(SHELF_ITEMS) {
            let app = self.clone();
            let open = playlist.clone();
            let card = self.card(&playlist.name, "Playlist", playlist.image.as_ref(), false, move || app.open_playlist(&open));
            flow.append(&card);
        }
        show_shelf(flow);
    }

    pub(super) async fn search(self: Rc<Self>, query: String) {
        let generation = self.next_generation();
        if query.chars().count() < 2 {
            self.search_results.set_child(Some(&search_placeholder()));
            return;
        }
        self.search_results.set_child(Some(&scrolled_clamp(&skeleton_rows(8), 900)));
        let backend = self.backend.clone();
        let request = query.clone();
        let result = tokio(async move { backend.search(request).await }).await;
        if self.generation.get() != generation {
            return;
        }
        match result {
            Ok(results) => self.show_search(results),
            Err(e) => {
                let app = Rc::downgrade(&self);
                self.search_results.set_child(Some(&error_page("Search Failed", &e, move || {
                    if let Some(app) = app.upgrade() {
                        glib::spawn_future_local(app.clone().search(query.clone()));
                    }
                })));
            }
        }
    }

    fn show_search(self: &Rc<Self>, results: SearchResults) {
        let SearchResults { tracks, artists, albums, playlists } = results;
        if tracks.is_empty() && artists.is_empty() && albums.is_empty() && playlists.is_empty() {
            self.search_results.set_child(Some(
                &adw::StatusPage::builder().icon_name("system-search-symbolic").title("No Results").description("Try other words").vexpand(true).build(),
            ));
            return;
        }
        let list = self.song_list(RowOptions { album: true, ..RowOptions::default() }, "Search", None);
        if !artists.is_empty() {
            let (section, flow) = shelf_section("Artists");
            self.fill_artist_shelf(&flow, artists.into_iter().take(6).collect());
            section.set_margin_top(12);
            list.push_leading(&section);
        }
        if !tracks.is_empty() {
            list.push_leading(&section_title("Songs"));
        }
        list.append(tracks);
        for (title, albums, playlists) in [("Albums", albums, Vec::new()), ("Playlists", Vec::new(), playlists)] {
            if albums.is_empty() && playlists.is_empty() {
                continue;
            }
            let (section, flow) = shelf_section(title);
            if playlists.is_empty() {
                self.fill_album_shelf(&flow, albums);
            } else {
                self.fill_playlist_shelf(&flow, playlists);
            }
            section.set_margin_top(18);
            list.push_trailing(&section);
        }
        self.search_results.set_child(Some(&list.ready()));
    }
}

/// A titled grid of cards, showing placeholders until it is filled.
fn shelf_section(title: &str) -> (gtk::Box, gtk::FlowBox) {
    let flow = gtk::FlowBox::builder()
        .homogeneous(true)
        .selection_mode(gtk::SelectionMode::None)
        .min_children_per_line(2)
        .max_children_per_line(6)
        .column_spacing(6)
        .row_spacing(14)
        .css_classes(["shelf"])
        .build();
    for _ in 0..6 {
        let card = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).halign(gtk::Align::Center).margin_top(4).build();
        card.append(&gtk::Box::builder().width_request(CARD_ART_SIZE).height_request(CARD_ART_SIZE).css_classes(["skeleton"]).build());
        card.append(&gtk::Box::builder().width_request(110).height_request(12).halign(gtk::Align::Start).css_classes(["skeleton", "line"]).build());
        card.append(&gtk::Box::builder().width_request(70).height_request(10).halign(gtk::Align::Start).css_classes(["skeleton", "line"]).build());
        flow.append(&card);
    }
    let section = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).build();
    section.append(&gtk::Label::builder().label(title).xalign(0.0).css_classes(["title-2"]).build());
    section.append(&flow);
    (section, flow)
}

fn show_shelf(flow: &gtk::FlowBox) {
    if let Some(section) = flow.parent() {
        section.set_visible(flow.first_child().is_some());
    }
}

fn liked_tile() -> gtk::Widget {
    gtk::Image::builder()
        .icon_name("tune-heart-filled-symbolic")
        .pixel_size(72)
        .width_request(HEADER_ART_SIZE)
        .height_request(HEADER_ART_SIZE)
        .valign(gtk::Align::Center)
        .css_classes(["album-art", "huge", "liked-tile"])
        .build()
        .upcast()
}

/// Ties a page's loader (and its Try Again) to the page's lifetime.
fn stack_keepalive(page: &adw::NavigationPage, load: Rc<RefCell<Option<Rc<dyn Fn()>>>>) {
    page.connect_destroy(move |_| {
        load.replace(None);
    });
}
