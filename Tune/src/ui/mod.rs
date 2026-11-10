//! The window: a login page, then a sidebar (Home, Search, Liked Songs,
//! playlists), pages of songs on the right, and the player bar along the
//! bottom.
//!
//! - `player`: the player bar, the queue, playback, media controls,
//!   Spotify Connect, and the Now Playing and Queue pages.
//! - `pages`: Home, Search, and pages of songs (playlists, albums, artists).
//! - `songs`: song rows, the lists they sit in, and their menu.
//! - `prefs`: the Preferences and Keyboard Shortcuts dialogs.

mod lyrics;
mod mini;
mod pages;
mod player;
mod playlists;
mod prefs;
mod songs;
mod style;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::api::{Playlist, Track};
use crate::backend::Backend;
use crate::settings::Settings;
use crate::tokio;
use player::{Clock, NowPlaying, PlayerBar, Queue, QueuePage, Remote};
use songs::SongRow;

const SIDEBAR_ART_SIZE: i32 = 36;
const SIDEBAR_WIDTH: (f64, f64) = (220.0, 280.0);
/// Just the covers: art, row and list padding, and the theme's 7px pane margin.
const SIDEBAR_COMPACT_WIDTH: f64 = 70.0;
/// Seek steps for Shift+arrows.
const SEEK_STEP_MS: i64 = 10_000;
const VOLUME_STEP: f64 = 0.05;

#[derive(Clone)]
enum SidebarItem {
    Home,
    Search,
    Liked,
    Playlist(Playlist),
}

struct Sidebar {
    list: gtk::ListBox,
    items: RefCell<Vec<SidebarItem>>,
    /// Row labels, hidden in the compact (covers only) sidebar.
    labels: RefCell<Vec<gtk::Label>>,
    header: adw::HeaderBar,
    account: adw::WindowTitle,
    menu: gtk::MenuButton,
    /// The app's mark: beside the name, or alone and centred when compact.
    logo: gtk::Image,
    logo_centered: gtk::Image,
    compact: Cell<bool>,
}

struct App {
    backend: Arc<Backend>,
    application: adw::Application,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    pages: gtk::Stack,
    login_button: gtk::Button,
    split: adw::NavigationSplitView,
    sidebar: Sidebar,
    account: adw::WindowTitle,
    nav: adw::NavigationView,
    search_page: adw::NavigationPage,
    search_entry: gtk::SearchEntry,
    search_results: adw::Bin,
    /// Bumped whenever the visible content changes, so a slow load cannot
    /// replace a newer one.
    generation: Cell<u64>,
    bar: PlayerBar,
    queue: RefCell<Queue>,
    /// The song Tune itself is on (the queue's current one).
    current: RefCell<Option<Track>>,
    playing: Cell<bool>,
    /// False while the current song is only restored from last time, not
    /// yet handed to the player.
    loaded: Cell<bool>,
    /// Songs in a row that ended without ever playing; see `on_failed_track`.
    failures: Cell<u32>,
    clock: Cell<Clock>,
    pending_seek: RefCell<Option<glib::SourceId>>,
    images: RefCell<HashMap<String, gdk::Texture>>,
    mpris: RefCell<Option<Rc<mpris_server::Player>>>,
    /// Every song row on screen, to mark the one playing.
    rows: RefCell<Vec<Weak<SongRow>>>,
    sidebar_animation: RefCell<Option<adw::Animation>>,
    settings: RefCell<Settings>,
    playlists: RefCell<Vec<Playlist>>,
    /// Whether the song in the player bar is in Liked Songs; None until known.
    liked: Cell<Option<bool>>,
    /// Set while another Spotify app plays through Tune (Spotify Connect).
    remote: RefCell<Option<Remote>>,
    /// Wide window: song rows show their album and date added.
    wide: Cell<bool>,
    /// Colours taken from the cover playing.
    cover_css: gtk::CssProvider,
    now_playing: RefCell<Option<NowPlaying>>,
    queue_page: RefCell<Option<QueuePage>>,
    /// Keeps Tune running with its window hidden (background play).
    hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    /// Ticker count, for saving the position every so often.
    ticks: Cell<u32>,
    /// Ignores the player bar's own signals while Tune sets its controls.
    syncing: Cell<bool>,
    /// The sleep timer: a countdown, or stopping when the song ends.
    sleep_timer: RefCell<Option<glib::SourceId>>,
    sleep_at_end: Cell<bool>,
    lyrics: RefCell<Option<Rc<lyrics::LyricsView>>>,
    mini: RefCell<Option<mini::MiniPlayer>>,
}

pub fn activate(application: &adw::Application) {
    // A window hidden for background play comes back. The mini player is
    // a window too, but the main one is what is being asked for here.
    if let Some(window) = application.windows().iter().find(|window| !window.has_css_class("mini-window")) {
        window.present();
        return;
    }

    let provider = gtk::CssProvider::new();
    provider.load_from_string(style::CSS);
    // Tune's own icons: the app icon is in the user's hicolor theme
    // (install.sh), the hearts in Tune's data folder; both are in the
    // source tree too.
    let display = gdk::Display::default().expect("display");
    let icons = gtk::IconTheme::for_display(&display);
    icons.add_search_path(concat!(env!("CARGO_MANIFEST_DIR"), "/data/icons"));
    icons.add_search_path(glib::user_data_dir().join("tune").join("icons"));
    gtk::Window::set_default_icon_name(crate::APP_ID);
    // Above the user's own gtk.css (where the rice theme lives); the
    // selectors only reach Tune's own widgets.
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_USER + 1);
    let cover_css = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(&display, &cover_css, gtk::STYLE_PROVIDER_PRIORITY_USER + 2);

    let settings = Settings::load();
    let (updates_tx, updates_rx) = async_channel::unbounded();
    let backend = match Backend::new(updates_tx, settings.audio(), settings.connect) {
        Ok(backend) => backend,
        Err(e) => {
            log::error!("cannot start: {e}");
            return;
        }
    };
    std::thread::spawn(Backend::prune_image_cache);

    let app = build(application, backend, settings, cover_css);
    app.window.present();

    // Player events, the position ticker, media controls, and the stored login.
    glib::spawn_future_local({
        let app = app.clone();
        async move {
            while let Ok(update) = updates_rx.recv().await {
                app.on_player_update(update);
            }
        }
    });
    glib::timeout_add_local(Duration::from_millis(250), {
        let app = app.clone();
        move || {
            app.tick();
            glib::ControlFlow::Continue
        }
    });
    glib::spawn_future_local({
        let app = app.clone();
        async move { app.setup_mpris().await }
    });
    glib::spawn_future_local({
        let app = app.clone();
        async move { app.restore().await }
    });
    // For checking the interface: TUNE_SNAPSHOT_DIR=dir tune renders the
    // main screens to PNGs there, without playing anything.
    if let Some(dir) = std::env::var_os("TUNE_SNAPSHOT_DIR") {
        glib::spawn_future_local(app.clone().snapshot_tour(dir.into()));
    }
}

fn build(application: &adw::Application, backend: Arc<Backend>, settings: Settings, cover_css: gtk::CssProvider) -> Rc<App> {
    // Login page.
    let login_button = gtk::Button::builder()
        .label("Log in with Spotify")
        .halign(gtk::Align::Center)
        .css_classes(["pill", "suggested-action"])
        .build();
    let login = adw::StatusPage::builder()
        .icon_name(crate::APP_ID)
        .title("Tune")
        .description("Listen to Spotify. Logging in opens Spotify's own sign-in page in your browser; Spotify Premium is required.")
        .child(&login_button)
        .build();
    let login_view = adw::ToolbarView::new();
    login_view.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    login_view.set_content(Some(&login));

    let loading = adw::StatusPage::builder().title("Connecting to Spotify…").child(&adw::Spinner::new()).build();
    let loading_view = adw::ToolbarView::new();
    loading_view.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    loading_view.set_content(Some(&loading));

    // Sidebar.
    let sidebar_list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
    let account = adw::WindowTitle::new("Tune", "");
    let menu = gio::Menu::new();
    let section = gio::Menu::new();
    section.append(Some("Now Playing"), Some("app.now-playing"));
    section.append(Some("Lyrics"), Some("app.lyrics"));
    section.append(Some("Queue"), Some("app.queue"));
    section.append(Some("Mini Player"), Some("app.mini-player"));
    let sleep = gio::Menu::new();
    for (label, target) in [
        ("Off", "off"),
        ("In 15 Minutes", "15"),
        ("In 30 Minutes", "30"),
        ("In 45 Minutes", "45"),
        ("In 1 Hour", "60"),
        ("At the End of This Song", "end"),
    ] {
        sleep.append_item(&gio::MenuItem::new(Some(label), Some(&format!("app.sleep::{target}"))));
    }
    section.append_submenu(Some("Sleep Timer"), &sleep);
    menu.append_section(None, &section);
    let section = gio::Menu::new();
    section.append(Some("New Playlist…"), Some("app.new-playlist"));
    section.append(Some("Refresh Library"), Some("app.refresh"));
    section.append(Some("Copy Link to Song"), Some("app.copy-link"));
    menu.append_section(None, &section);
    let section = gio::Menu::new();
    section.append(Some("Preferences"), Some("app.preferences"));
    section.append(Some("Keyboard Shortcuts"), Some("app.shortcuts"));
    section.append(Some("About Tune"), Some("app.about"));
    menu.append_section(None, &section);
    let section = gio::Menu::new();
    section.append(Some("Log Out"), Some("app.logout"));
    section.append(Some("Quit"), Some("app.quit"));
    menu.append_section(None, &section);
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text("Menu")
        .margin_end(6)
        .build();
    let logo_icon = format!("{}-symbolic", crate::APP_ID);
    // Centred over the covers in the list below (list 12px + row 6px + half a cover).
    let logo = gtk::Image::builder().icon_name(&logo_icon).pixel_size(18).margin_start(25).build();
    let logo_centered = gtk::Image::builder().icon_name(&logo_icon).pixel_size(20).tooltip_text("Tune").build();
    let sidebar_header = adw::HeaderBar::builder().title_widget(&account).build();
    sidebar_header.pack_start(&logo);
    sidebar_header.pack_end(&menu_button);
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&sidebar_header);
    sidebar_view.set_content(Some(
        // Scrolls, but without a scrollbar drawn over the covers.
        &gtk::ScrolledWindow::builder()
            .child(&sidebar_list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .build(),
    ));

    // Search, kept for the whole session so the query and results stay.
    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text("Songs, artists, albums and playlists")
        .search_delay(300)
        .build();
    let search_results = adw::Bin::builder().vexpand(true).build();
    let search_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    search_box.append(
        &adw::Clamp::builder().maximum_size(900).child(&search_entry).margin_top(12).margin_start(12).margin_end(12).build(),
    );
    search_box.append(&search_results);
    search_results.set_child(Some(&pages::search_placeholder()));
    let (search_page, _) = content_page("Search", &search_box);

    // The player sits under the pages, in the content column: the theme's
    // sidebar is a card that runs the full height of the window.
    let nav = adw::NavigationView::new();
    let bar = player::build_player_bar(&settings);
    let content_view = adw::ToolbarView::new();
    content_view.set_content(Some(&nav));
    content_view.add_bottom_bar(&bar.root);
    content_view.set_bottom_bar_style(adw::ToolbarStyle::Flat);
    content_view.set_extend_content_to_bottom_edge(true);
    content_view.add_css_class("player-host");

    let split = adw::NavigationSplitView::builder()
        .sidebar(&adw::NavigationPage::new(&sidebar_view, "Tune"))
        .content(&adw::NavigationPage::builder().child(&content_view).title("Tune").build())
        .min_sidebar_width(SIDEBAR_WIDTH.0)
        .max_sidebar_width(SIDEBAR_WIDTH.1)
        .build();

    let pages = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
    pages.add_named(&loading_view, Some("loading"));
    pages.add_named(&login_view, Some("login"));
    pages.add_named(&split, Some("main"));

    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&pages));

    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title("Tune")
        .default_width(1100)
        .default_height(740)
        .width_request(360)
        .height_request(400)
        .content(&toasts)
        .build();

    let app = Rc::new(App {
        backend,
        application: application.clone(),
        window,
        toasts,
        pages,
        login_button,
        split,
        sidebar: Sidebar {
            list: sidebar_list,
            items: RefCell::new(Vec::new()),
            labels: RefCell::new(Vec::new()),
            header: sidebar_header,
            account: account.clone(),
            menu: menu_button,
            logo,
            logo_centered,
            compact: Cell::new(false),
        },
        account,
        nav,
        search_page,
        search_entry,
        search_results,
        generation: Cell::new(0),
        bar,
        queue: RefCell::new(Queue::default()),
        current: RefCell::new(None),
        playing: Cell::new(false),
        loaded: Cell::new(false),
        failures: Cell::new(0),
        clock: Cell::new(Clock::default()),
        pending_seek: RefCell::new(None),
        images: RefCell::new(HashMap::new()),
        mpris: RefCell::new(None),
        rows: RefCell::new(Vec::new()),
        sidebar_animation: RefCell::new(None),
        settings: RefCell::new(settings),
        playlists: RefCell::new(Vec::new()),
        liked: Cell::new(None),
        remote: RefCell::new(None),
        wide: Cell::new(false),
        cover_css,
        now_playing: RefCell::new(None),
        queue_page: RefCell::new(None),
        hold: RefCell::new(None),
        ticks: Cell::new(0),
        syncing: Cell::new(false),
        sleep_timer: RefCell::new(None),
        sleep_at_end: Cell::new(false),
        lyrics: RefCell::new(None),
        mini: RefCell::new(None),
    });
    app.add_breakpoints();
    app.connect_signals();
    app.connect_player_bar();
    app
}

/// The sidebar toggle in every content header: one stateful action keeps
/// them all in step.
/// "Playlists" over the sidebar's playlists, with a button to make one.
fn playlists_heading() -> gtk::Box {
    let heading = gtk::Box::builder().margin_top(14).margin_bottom(4).margin_start(10).margin_end(4).build();
    heading.append(
        &gtk::Label::builder().label("Playlists").xalign(0.0).hexpand(true).css_classes(["heading", "dim-label"]).build(),
    );
    heading.append(
        &gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("New Playlist")
            .action_name("app.new-playlist")
            .css_classes(["flat", "circular"])
            .build(),
    );
    heading
}

fn sidebar_toggle() -> gtk::ToggleButton {
    gtk::ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text("Show Only Covers")
        .action_name("win.compact-sidebar")
        .build()
}

/// A page for the content side: a header with a title, over `child`.
fn content_page(title: &str, child: &impl IsA<gtk::Widget>) -> (adw::NavigationPage, adw::WindowTitle) {
    let window_title = adw::WindowTitle::new(title, "");
    let header = adw::HeaderBar::builder().title_widget(&window_title).build();
    header.pack_start(&sidebar_toggle());
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(child));
    (adw::NavigationPage::new(&view, title), window_title)
}

/// Room under scrolling pages for the floating player; taller when the
/// player stacks in a narrow window.
fn player_clearance() -> gtk::Box {
    gtk::Box::builder().css_classes(["player-clearance"]).height_request(player::clearance_height()).build()
}

fn scrolled_clamp(child: &impl IsA<gtk::Widget>, width: i32) -> gtk::ScrolledWindow {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.append(child);
    column.append(&player_clearance());
    let clamp = adw::Clamp::builder().maximum_size(width).child(&column).margin_top(12).margin_start(12).margin_end(12).build();
    gtk::ScrolledWindow::builder().child(&clamp).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build()
}

fn art_image(size: i32, classes: &[&str]) -> gtk::Image {
    let image = gtk::Image::builder()
        .icon_name("audio-x-generic-symbolic")
        .pixel_size(size.min(48))
        .width_request(size)
        .height_request(size)
        .overflow(gtk::Overflow::Hidden)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .css_classes(["album-art", "placeholder"])
        .build();
    for class in classes {
        image.add_css_class(class);
    }
    image
}

fn format_time(ms: u32) -> String {
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// "1 hr 12 min", "38 min".
fn format_length(ms: u64) -> String {
    let minutes = ms / 60_000;
    if minutes >= 60 { format!("{} hr {} min", minutes / 60, minutes % 60) } else { format!("{minutes} min") }
}

fn count_label(count: usize) -> String {
    if count == 1 { "1 song".into() } else { format!("{count} songs") }
}

/// Markup with each artist as a link; plain names when there are no IDs.
fn artist_links(track: &Track) -> String {
    if track.artist_refs.is_empty() {
        return glib::markup_escape_text(&track.artists).into();
    }
    track
        .artist_refs
        .iter()
        .map(|artist| format!("<a href=\"artist:{}\">{}</a>", artist.id, glib::markup_escape_text(&artist.name)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn album_link(track: &Track, text: &str) -> String {
    match &track.album_id {
        Some(id) => format!("<a href=\"album:{id}\">{}</a>", glib::markup_escape_text(text)),
        None => glib::markup_escape_text(text).into(),
    }
}

/// Puts a cover in place; one that just arrived fades in.
fn show_art(image: &gtk::Image, texture: &gdk::Texture, size: i32, fade: bool) {
    image.set_paintable(Some(texture));
    image.set_pixel_size(size);
    image.remove_css_class("placeholder");
    if fade && image.is_mapped() {
        adw::TimedAnimation::new(image, 0.0, 1.0, 220, adw::PropertyAnimationTarget::new(image, "opacity")).play();
    }
}

// The sidebar's compact mode is remembered in ~/.config/tune/sidebar-compact.
fn compact_sidebar_path() -> std::path::PathBuf {
    glib::user_config_dir().join("tune").join("sidebar-compact")
}

fn load_compact_sidebar() -> bool {
    compact_sidebar_path().exists()
}

fn save_compact_sidebar(compact: bool) {
    let path = compact_sidebar_path();
    if compact {
        let _ = std::fs::create_dir_all(path.parent().expect("parent")).and_then(|()| std::fs::write(&path, ""));
    } else {
        let _ = std::fs::remove_file(path);
    }
}

impl App {
    fn add_breakpoints(self: &Rc<Self>) {
        let window = &self.window;
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 680sp").expect("condition"));
        narrow.add_setter(&self.split, "collapsed", Some(&true.to_value()));
        narrow.add_setter(&self.bar.volume_box, "visible", Some(&false.to_value()));
        narrow.add_setter(&self.bar.now_playing, "width-request", Some(&0.to_value()));
        narrow.add_setter(&self.bar.right, "width-request", Some(&0.to_value()));
        window.add_breakpoint(narrow);

        // Very narrow: the player stacks the song over the controls.
        let stacked = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 520sp").expect("condition"));
        stacked.add_setter(&self.split, "collapsed", Some(&true.to_value()));
        stacked.add_setter(&self.bar.volume_box, "visible", Some(&false.to_value()));
        stacked.add_setter(&self.bar.now_playing, "width-request", Some(&0.to_value()));
        stacked.add_setter(&self.bar.right, "width-request", Some(&0.to_value()));
        stacked.add_setter(&self.bar.root, "orientation", Some(&gtk::Orientation::Vertical.to_value()));
        stacked.connect_apply({
            let app = Rc::downgrade(self);
            move |_| {
                if let Some(app) = app.upgrade() {
                    app.bar.root.add_css_class("stacked");
                    app.set_clearance(player::CLEARANCE_STACKED);
                }
            }
        });
        stacked.connect_unapply({
            let app = Rc::downgrade(self);
            move |_| {
                if let Some(app) = app.upgrade() {
                    app.bar.root.remove_css_class("stacked");
                    app.set_clearance(player::CLEARANCE);
                }
            }
        });
        window.add_breakpoint(stacked);

        // Wide: song rows have room for the album and the date added.
        let wide = adw::Breakpoint::new(adw::BreakpointCondition::parse("min-width: 1000sp").expect("condition"));
        wide.connect_apply({
            let app = Rc::downgrade(self);
            move |_| {
                if let Some(app) = app.upgrade() {
                    app.set_wide(true);
                }
            }
        });
        wide.connect_unapply({
            let app = Rc::downgrade(self);
            move |_| {
                if let Some(app) = app.upgrade() {
                    app.set_wide(false);
                }
            }
        });
        window.add_breakpoint(wide);
    }

    /// Every page's spacer under its content follows the player's height.
    fn set_clearance(&self, height: i32) {
        fn walk(widget: &gtk::Widget, height: i32) {
            if widget.has_css_class("player-clearance") {
                widget.set_height_request(height);
            }
            let mut child = widget.first_child();
            while let Some(current) = child {
                walk(&current, height);
                child = current.next_sibling();
            }
        }
        player::set_clearance_height(height);
        walk(self.window.upcast_ref(), height);
    }

    fn add_action(self: &Rc<Self>, name: &str, accels: &[&str], f: impl Fn(&Rc<App>) + 'static) {
        let action = gio::SimpleAction::new(name, None);
        let app = Rc::downgrade(self);
        action.connect_activate(move |_, _| {
            if let Some(app) = app.upgrade() {
                f(&app);
            }
        });
        self.application.add_action(&action);
        if !accels.is_empty() {
            self.application.set_accels_for_action(&format!("app.{name}"), accels);
        }
    }

    fn connect_signals(self: &Rc<Self>) {
        self.login_button.connect_clicked({
            let app = self.clone();
            move |_| {
                let app = app.clone();
                glib::spawn_future_local(async move { app.login().await });
            }
        });

        self.add_action("logout", &[], |app| {
            let app = app.clone();
            glib::spawn_future_local(async move { app.logout().await });
        });
        self.add_action("preferences", &["<Ctrl>comma"], |app| app.show_preferences());
        self.add_action("shortcuts", &["<Ctrl>question"], |app| app.show_shortcuts());
        self.add_action("quit", &["<Ctrl>q"], |app| app.quit());
        self.add_action("search", &["<Ctrl>f"], |app| {
            if let Some(index) = app.sidebar.items.borrow().iter().position(|item| matches!(item, SidebarItem::Search)) {
                app.sidebar.list.select_row(app.sidebar.list.row_at_index(index as i32).as_ref());
            }
            app.open_sidebar_item(1);
        });
        self.add_action("mute", &["<Ctrl>m"], |app| app.bar.mute.set_active(!app.bar.mute.is_active()));
        self.add_action("like", &["<Ctrl>l"], |app| app.toggle_like());
        self.add_action("shuffle", &["<Ctrl>s"], |app| app.bar.shuffle.set_active(!app.bar.shuffle.is_active()));
        self.add_action("repeat", &["<Ctrl>r"], |app| app.cycle_repeat());
        self.add_action("queue", &["<Ctrl>u"], |app| app.toggle_queue_page());
        self.add_action("now-playing", &["<Ctrl>p"], |app| app.toggle_now_playing());
        self.add_action("lyrics", &["<Ctrl>y"], |app| app.toggle_lyrics());
        self.add_action("mini-player", &["<Ctrl><Shift>m"], |app| app.toggle_mini_player());
        self.add_action("new-playlist", &["<Ctrl>n"], |app| app.ask_new_playlist(None));
        self.add_action("about", &[], |app| app.show_about());
        self.add_action("refresh", &["F5"], |app| {
            let app = app.clone();
            glib::spawn_future_local(async move { app.refresh_library().await });
        });
        self.add_action("copy-link", &[], |app| {
            if let Some(track) = app.shown_track() {
                let id = track.uri.rsplit(':').next().unwrap_or_default();
                app.window.clipboard().set_text(&format!("https://open.spotify.com/track/{id}"));
                app.toast("Link copied");
            }
        });
        let sleep = gio::SimpleAction::new_stateful("sleep", Some(glib::VariantTy::STRING), &"off".to_variant());
        sleep.connect_change_state({
            let app = Rc::downgrade(self);
            move |action, state| {
                let (Some(app), Some(choice)) = (app.upgrade(), state.and_then(|s| s.str().map(str::to_owned))) else { return };
                action.set_state(&choice.to_variant());
                app.set_sleep_timer(&choice);
            }
        });
        self.application.add_action(&sleep);

        self.sidebar.list.connect_row_activated({
            let app = self.clone();
            move |_, row| app.open_sidebar_item(row.index())
        });
        let compact = gio::SimpleAction::new_stateful("compact-sidebar", None, &load_compact_sidebar().to_variant());
        compact.connect_change_state({
            let app = self.clone();
            move |action, state| {
                if let Some(state) = state.and_then(bool::from_variant) {
                    action.set_state(&state.to_variant());
                    app.set_compact_sidebar(state);
                }
            }
        });
        self.sidebar.compact.set(load_compact_sidebar());
        self.window.add_action(&compact);
        self.search_entry.connect_search_changed({
            let app = self.clone();
            move |entry| {
                let app = app.clone();
                let query = entry.text().trim().to_owned();
                glib::spawn_future_local(async move { app.search(query).await });
            }
        });

        // Playback keys, like Spotify's, unless a text field has the keyboard.
        // Capture phase: a focused button must not also react to the same Space.
        let keys = gtk::EventControllerKey::builder().propagation_phase(gtk::PropagationPhase::Capture).build();
        keys.connect_key_pressed({
            let app = self.clone();
            move |_, key, _, modifiers| {
                let typing = GtkWindowExt::focus(&app.window).is_some_and(|w| w.is::<gtk::Text>());
                if typing || !app.has_track() {
                    return glib::Propagation::Proceed;
                }
                let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
                let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
                match key {
                    gdk::Key::space if !ctrl && !shift => app.toggle_play(),
                    gdk::Key::Right if ctrl => app.next(),
                    gdk::Key::Left if ctrl => app.previous(),
                    gdk::Key::Right if shift => app.seek_by(SEEK_STEP_MS),
                    gdk::Key::Left if shift => app.seek_by(-SEEK_STEP_MS),
                    gdk::Key::Up if ctrl => app.bar.volume.set_value(app.bar.volume.value() + VOLUME_STEP),
                    gdk::Key::Down if ctrl => app.bar.volume.set_value(app.bar.volume.value() - VOLUME_STEP),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        self.window.add_controller(keys);

        self.window.connect_close_request({
            let app = self.clone();
            move |window| {
                app.save_state();
                // Background play: the window goes, the music stays.
                if app.settings.borrow().background && app.playing.get() {
                    app.hold.replace(Some(app.application.hold()));
                    window.set_visible(false);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        });
        self.window.connect_visible_notify({
            let app = Rc::downgrade(self);
            move |window| {
                if window.is_visible()
                    && let Some(app) = app.upgrade()
                {
                    app.hold.replace(None);
                    app.hide_mini();
                }
            }
        });
        // Quitting from the dock or the top bar skips the close request;
        // this is the last word on what was playing.
        self.application.connect_shutdown({
            let app = self.clone();
            move |_| app.save_state()
        });
    }

    fn quit(&self) {
        self.save_state();
        self.hold.replace(None);
        self.application.quit();
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }

    fn set_settings(&self, f: impl FnOnce(&mut Settings)) {
        let mut settings = self.settings.borrow_mut();
        f(&mut settings);
        settings.save();
    }

    // Login

    async fn restore(self: Rc<Self>) {
        let backend = self.backend.clone();
        match tokio(async move { backend.restore().await }).await {
            Ok(Some(name)) => self.enter_main(name).await,
            Ok(None) => self.pages.set_visible_child_name("login"),
            Err(e) => {
                self.pages.set_visible_child_name("login");
                self.toast(&e);
            }
        }
    }

    async fn login(self: Rc<Self>) {
        self.login_button.set_sensitive(false);
        self.login_button.set_label("Waiting for the browser…");
        let backend = self.backend.clone();
        let result = tokio(async move { backend.login().await }).await;
        self.login_button.set_sensitive(true);
        self.login_button.set_label("Log in with Spotify");
        match result {
            Ok(name) => {
                self.window.present();
                self.enter_main(name).await;
            }
            Err(e) => self.toast(&e),
        }
    }

    async fn logout(self: Rc<Self>) {
        let backend = self.backend.clone();
        tokio(async move { backend.logout().await }).await;
        self.queue.replace(Queue::default());
        self.current.replace(None);
        self.remote.replace(None);
        self.set_playing(false, 0);
        self.save_state();
        self.reset_bar();
        self.pages.set_visible_child_name("login");
    }

    async fn enter_main(self: &Rc<Self>, name: String) {
        self.account.set_subtitle(&name);
        self.pages.set_visible_child_name("main");
        self.load_sidebar().await;
        if let Some(home) = self.sidebar.list.row_at_index(0) {
            self.sidebar.list.select_row(Some(&home));
        }
        if self.current.borrow().is_none() {
            self.restore_state();
        }
        self.open_sidebar_item(0);
    }

    /// Playlists again, for ones made or renamed in another Spotify app;
    /// the page showing stays.
    async fn refresh_library(self: Rc<Self>) {
        if self.pages.visible_child_name().as_deref() != Some("main") {
            return;
        }
        let selected = self.sidebar.list.selected_row().map(|row| row.index()).unwrap_or(0);
        self.load_sidebar().await;
        self.sidebar.list.select_row(self.sidebar.list.row_at_index(selected).as_ref());
        self.toast("Library refreshed");
    }

    async fn load_sidebar(self: &Rc<Self>) {
        let backend = self.backend.clone();
        let playlists = tokio(async move { backend.playlists().await }).await.unwrap_or_else(|e| {
            self.toast(&format!("Could not load playlists: {e}"));
            Vec::new()
        });
        self.playlists.replace(playlists.clone());

        let list = &self.sidebar.list;
        list.remove_all();
        self.sidebar.labels.borrow_mut().clear();
        let mut items = vec![SidebarItem::Home, SidebarItem::Search, SidebarItem::Liked];
        items.extend(playlists.into_iter().map(SidebarItem::Playlist));
        // A small heading above the playlists, unless only covers are shown.
        // A list drops headers it has no function for.
        list.set_header_func(|row, _| {
            let wanted = row.index() == 3 && row.parent().is_some_and(|list| !list.has_css_class("compact"));
            if !wanted {
                row.set_header(gtk::Widget::NONE);
            } else if row.header().is_none() {
                row.set_header(Some(&playlists_heading()));
            }
        });
        for item in &items {
            list.append(&self.sidebar_row(item));
        }
        self.sidebar.items.replace(items);
        self.set_compact_sidebar(self.sidebar.compact.get());
    }

    /// Playlists songs can be added to: the user's own and collaborative ones.
    fn editable_playlists(&self) -> Vec<Playlist> {
        let me = self.backend.user_id();
        self.playlists.borrow().iter().filter(|p| p.collaborative || (!me.is_empty() && p.owner_id == me)).cloned().collect()
    }

    fn sidebar_row(self: &Rc<Self>, item: &SidebarItem) -> gtk::ListBoxRow {
        let tile = |icon: &str, class: &str| {
            let tile = gtk::Image::builder()
                .icon_name(icon)
                .width_request(SIDEBAR_ART_SIZE)
                .height_request(SIDEBAR_ART_SIZE)
                .css_classes(["sidebar-tile", class])
                .build();
            tile.upcast::<gtk::Widget>()
        };
        let (cover, name) = match item {
            SidebarItem::Home => (tile("go-home-symbolic", "icon-tile"), "Home".to_owned()),
            SidebarItem::Search => (tile("system-search-symbolic", "icon-tile"), "Search".to_owned()),
            SidebarItem::Liked => (tile("starred-symbolic", "liked-tile"), "Liked Songs".to_owned()),
            SidebarItem::Playlist(playlist) => {
                let art = art_image(SIDEBAR_ART_SIZE, &[]);
                art.set_pixel_size(16);
                if let Some(url) = &playlist.image {
                    self.load_image(&art, url.clone(), SIDEBAR_ART_SIZE);
                }
                (art.upcast(), playlist.name.clone())
            }
        };
        let label = gtk::Label::builder().label(&name).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
        let content = gtk::Box::builder().spacing(12).build();
        content.append(&cover);
        content.append(&label);
        // Centred under the cover when the labels are hidden.
        label
            .bind_property("visible", &content, "halign")
            .transform_to(|_, visible: bool| Some(if visible { gtk::Align::Fill } else { gtk::Align::Center }))
            .sync_create()
            .build();
        self.sidebar.labels.borrow_mut().push(label);
        gtk::ListBoxRow::builder().child(&content).tooltip_text(&name).build()
    }

    /// Covers only, like Spotify's collapsed library. The width slides;
    /// names go away before shrinking and come back after growing.
    fn set_compact_sidebar(&self, compact: bool) {
        let sidebar = &self.sidebar;
        let changed = sidebar.compact.replace(compact) != compact;
        save_compact_sidebar(compact);

        let show_names = {
            let labels = sidebar.labels.borrow().clone();
            let (list, header, menu, account) =
                (sidebar.list.clone(), sidebar.header.clone(), sidebar.menu.clone(), sidebar.account.clone());
            let (logo, logo_centered) = (sidebar.logo.clone(), sidebar.logo_centered.clone());
            move |show: bool| {
                for label in &labels {
                    label.set_visible(show);
                }
                // Compact rows shrink to the cover, so the highlight is a
                // square with even padding all round.
                let mut row = list.first_child();
                while let Some(widget) = row {
                    widget.set_halign(if show { gtk::Align::Fill } else { gtk::Align::Center });
                    row = widget.next_sibling();
                }
                if show {
                    list.remove_css_class("compact");
                    list.invalidate_headers();
                } else {
                    list.add_css_class("compact");
                    list.invalidate_headers();
                }
                // Compact: only the app's mark, centred at the top.
                logo.set_visible(show);
                menu.set_visible(show);
                if show {
                    header.set_title_widget(Some(&account));
                } else {
                    header.set_title_widget(Some(&logo_centered));
                }
            }
        };
        if compact {
            show_names(false);
        }

        let from = (self.split.min_sidebar_width(), self.split.max_sidebar_width());
        let to = if compact { (SIDEBAR_COMPACT_WIDTH, SIDEBAR_COMPACT_WIDTH) } else { SIDEBAR_WIDTH };
        let split = self.split.clone();
        let set_width = move |t: f64| {
            let min = from.0 + (to.0 - from.0) * t;
            let max = from.1 + (to.1 - from.1) * t;
            // Keep min <= max at every step.
            if compact {
                split.set_min_sidebar_width(min);
                split.set_max_sidebar_width(max);
            } else {
                split.set_max_sidebar_width(max);
                split.set_min_sidebar_width(min);
            }
        };
        if !changed || !self.window.is_mapped() {
            set_width(1.0);
            show_names(!compact);
            return;
        }
        let animation = adw::TimedAnimation::builder()
            .widget(&self.split)
            .value_from(0.0)
            .value_to(1.0)
            .duration(260)
            .easing(adw::Easing::EaseOutCubic)
            .target(&adw::CallbackAnimationTarget::new(set_width))
            .build();
        if !compact {
            animation.connect_done(move |_| show_names(true));
        }
        animation.play();
        self.sidebar_animation.replace(Some(animation.upcast()));
    }

    fn next_generation(&self) -> u64 {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        generation
    }

    fn load_image(self: &Rc<Self>, image: &gtk::Image, url: String, size: i32) {
        self.load_image_if(image, url, size, || true);
    }

    /// Like `load_image`, for widgets that get reused: `still_wanted` is
    /// asked once the cover arrives, in case the row shows another song by then.
    fn load_image_if(self: &Rc<Self>, image: &gtk::Image, url: String, size: i32, still_wanted: impl Fn() -> bool + 'static) {
        if let Some(texture) = self.images.borrow().get(&url) {
            show_art(image, texture, size, false);
            return;
        }
        let app = self.clone();
        let image = image.downgrade();
        glib::spawn_future_local(async move {
            let Some(texture) = app.texture(url).await else { return };
            if let Some(image) = image.upgrade()
                && still_wanted()
            {
                show_art(&image, &texture, size, true);
            }
        });
    }

    /// A cover, from memory, the disk cache or Spotify.
    async fn texture(self: &Rc<Self>, url: String) -> Option<gdk::Texture> {
        if let Some(texture) = self.images.borrow().get(&url) {
            return Some(texture.clone());
        }
        let backend = self.backend.clone();
        let fetch = url.clone();
        let bytes = tokio(async move { backend.image(fetch).await }).await?;
        let texture = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok()?;
        self.images.borrow_mut().insert(url, texture.clone());
        Some(texture)
    }

    /// Opens an "artist:ID" or "album:ID" link from a label.
    fn open_link(self: &Rc<Self>, link: &str) -> glib::Propagation {
        if let Some(id) = link.strip_prefix("artist:") {
            self.open_artist(id.to_owned());
        } else if let Some(id) = link.strip_prefix("album:") {
            self.open_album(id.to_owned());
        } else {
            return glib::Propagation::Proceed;
        }
        glib::Propagation::Stop
    }

    // Snapshots (see TUNE_SNAPSHOT_DIR)

    fn snapshot(&self, path: &std::path::Path) {
        let window = &self.window;
        let paintable = gtk::WidgetPaintable::new(Some(window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, f64::from(window.width()), f64::from(window.height()));
        let (Some(node), Some(renderer)) = (snapshot.to_node(), window.renderer()) else { return };
        if let Err(e) = renderer.render_texture(&node, None).save_to_png(path) {
            log::warn!("snapshot {}: {e}", path.display());
        }
    }

    async fn snapshot_tour(self: Rc<Self>, dir: std::path::PathBuf) {
        let wait = |seconds| glib::timeout_future(Duration::from_secs(seconds));
        let _ = std::fs::create_dir_all(&dir);
        // After login and the library have loaded.
        while self.pages.visible_child_name().as_deref() != Some("main") || self.sidebar.list.row_at_index(3).is_none() {
            wait(1).await;
        }
        wait(4).await;
        self.snapshot(&dir.join("1-home.png"));
        self.sidebar.list.select_row(self.sidebar.list.row_at_index(2).as_ref());
        self.open_sidebar_item(2);
        wait(5).await;
        self.snapshot(&dir.join("2-liked.png"));
        if let Some(scroller) = self.nav.visible_page().and_then(|page| find_scrolled(page.upcast_ref())) {
            scroller.vadjustment().set_value(400.0);
            wait(1).await;
            self.snapshot(&dir.join("2b-liked-scrolled.png"));
            scroller.vadjustment().set_value(0.0);
        }
        if self.sidebar.list.row_at_index(3).is_some() {
            self.sidebar.list.select_row(self.sidebar.list.row_at_index(3).as_ref());
            self.open_sidebar_item(3);
            wait(5).await;
            self.snapshot(&dir.join("2c-playlist.png"));
        }
        let was_compact = self.sidebar.compact.get();
        self.window.change_action_state("compact-sidebar", &(!was_compact).to_variant());
        wait(1).await;
        self.snapshot(&dir.join("3-sidebar-toggled.png"));
        self.window.change_action_state("compact-sidebar", &was_compact.to_variant());
        self.sidebar.list.select_row(self.sidebar.list.row_at_index(1).as_ref());
        self.open_sidebar_item(1);
        self.search_entry.set_text("lana del rey");
        wait(5).await;
        self.snapshot(&dir.join("4-search.png"));
        let backend = self.backend.clone();
        let found = tokio(async move { backend.search("lana del rey".into()).await }).await.ok();
        let first = found.as_ref().and_then(|r| r.tracks.first().cloned());
        let artist = first.as_ref().and_then(|t| t.artist_refs.first().map(|a| a.id.clone()));
        if let Some(id) = artist {
            self.open_artist(id);
            wait(5).await;
            self.snapshot(&dir.join("5-artist.png"));
        }
        let album = first.as_ref().and_then(|t| t.album_id.clone());
        if let Some(id) = album {
            self.open_album(id);
            wait(4).await;
            self.snapshot(&dir.join("6-album.png"));
        }
        if self.current.borrow().is_some() {
            self.toggle_now_playing();
            wait(2).await;
            self.snapshot(&dir.join("7-now-playing.png"));
            self.toggle_queue_page();
            wait(2).await;
            self.snapshot(&dir.join("8-queue.png"));
            self.toggle_lyrics();
            wait(5).await;
            self.snapshot(&dir.join("9-lyrics.png"));
        }
        self.search_entry.set_text("");
        self.sidebar.list.select_row(self.sidebar.list.row_at_index(0).as_ref());
        self.open_sidebar_item(0);
        log::info!("snapshots saved in {}", dir.display());
    }
}

/// The first visible scrolled window inside `widget`, depth first.
fn find_scrolled(widget: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
    // Only what is on screen: a stack's hidden pages have scrollers of their own.
    if !widget.is_mapped() {
        return None;
    }
    if let Some(scrolled) = widget.downcast_ref::<gtk::ScrolledWindow>() {
        return Some(scrolled.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = find_scrolled(&current) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}
