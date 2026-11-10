//! The Lyrics page: LRCLIB's lyrics for the song in the player bar. Synced
//! lyrics light up line by line and keep the current one in view; clicking
//! a line jumps the song there. Plain lyrics are shown as they are.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::glib;

use super::App;
use crate::lyrics::Lyrics;
use crate::tokio;

/// After the user scrolls, the view leaves the scrolling to them this long.
const USER_SCROLL_PAUSE: Duration = Duration::from_secs(4);
const OFFSET_STEP_MS: i32 = 250;
/// Where the current line sits, as a share of the view's height.
const CURRENT_LINE_AT: f64 = 0.35;

pub struct LyricsView {
    page: adw::NavigationPage,
    stack: gtk::Stack,
    scrolled: gtk::ScrolledWindow,
    lines_box: gtk::Box,
    message: adw::StatusPage,
    /// Synced lines: start time and widget. Empty for plain lyrics.
    lines: RefCell<Vec<(u32, gtk::Widget)>>,
    current: Cell<Option<usize>>,
    /// The song whose lyrics are shown or loading.
    uri: RefCell<Option<String>>,
    /// Added to the song's position: positive shows lines earlier.
    offset_ms: Cell<i32>,
    offset_label: gtk::Label,
    user_scrolled: Cell<Option<Instant>>,
    animation: RefCell<Option<adw::TimedAnimation>>,
}

impl LyricsView {
    pub fn page(&self) -> adw::NavigationPage {
        self.page.clone()
    }

    fn show_offset(&self) {
        let offset = self.offset_ms.get();
        self.offset_label.set_label(&if offset == 0 { "Synced".to_owned() } else { format!("{:+.2} s", f64::from(offset) / 1000.0) });
        if offset == 0 {
            self.offset_label.remove_css_class("shifted");
        } else {
            self.offset_label.add_css_class("shifted");
        }
    }
}

impl App {
    pub(super) fn toggle_lyrics(self: &Rc<Self>) {
        if self.shown_track().is_none() {
            return;
        }
        let view = self.lyrics_view();
        self.toggle_page(&view.page);
        self.load_lyrics();
    }

    fn lyrics_view(self: &Rc<Self>) -> Rc<LyricsView> {
        if let Some(view) = &*self.lyrics.borrow() {
            return view.clone();
        }
        let lines_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            // Room for the first and last lines to reach the middle.
            .margin_top(120)
            .margin_bottom(240)
            .build();
        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        column.append(&adw::Clamp::builder().maximum_size(760).child(&lines_box).margin_start(24).margin_end(24).build());
        column.append(&super::player_clearance());
        let scrolled = gtk::ScrolledWindow::builder().child(&column).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();

        let message = adw::StatusPage::builder().vexpand(true).build();
        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).css_classes(["now-playing-view", "lyrics-view"]).build();
        stack.add_named(&adw::Spinner::builder().width_request(32).height_request(32).halign(gtk::Align::Center).valign(gtk::Align::Center).build(), Some("loading"));
        stack.add_named(&scrolled, Some("lines"));
        stack.add_named(&message, Some("message"));

        // Timing nudges, for the odd entry that runs early or late.
        let offset_label = gtk::Label::builder().width_chars(7).css_classes(["caption", "numeric"]).build();
        let later = gtk::Button::builder().icon_name("list-remove-symbolic").tooltip_text("Show Lyrics Later").css_classes(["flat", "circular"]).build();
        let earlier = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text("Show Lyrics Earlier").css_classes(["flat", "circular"]).build();
        let offset = gtk::Box::builder().spacing(2).css_classes(["timing-pill"]).valign(gtk::Align::Center).build();
        offset.append(&later);
        offset.append(&gtk::Button::builder().child(&offset_label).tooltip_text("Reset Timing").css_classes(["flat", "timing-value"]).build());
        offset.append(&earlier);

        let (page, _) = super::content_page("Lyrics", &stack);
        page.set_tag(Some("lyrics"));
        if let Some(header) = page.child().and_downcast::<adw::ToolbarView>().and_then(|view| find_header(view.upcast_ref())) {
            header.pack_end(&offset);
        }

        let view = Rc::new(LyricsView {
            page,
            stack,
            scrolled,
            lines_box,
            message,
            lines: RefCell::default(),
            current: Cell::new(None),
            uri: RefCell::new(None),
            offset_ms: Cell::new(0),
            offset_label,
            user_scrolled: Cell::new(None),
            animation: RefCell::new(None),
        });
        view.show_offset();

        let nudge = |button: &gtk::Button, step: Option<i32>| {
            let view = Rc::downgrade(&view);
            let app = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                let (Some(view), Some(app)) = (view.upgrade(), app.upgrade()) else { return };
                view.offset_ms.set(step.map_or(0, |step| view.offset_ms.get() + step));
                view.show_offset();
                view.user_scrolled.set(None);
                app.lyrics_tick(app.clock.get().now_ms());
            });
        };
        nudge(&later, Some(-OFFSET_STEP_MS));
        nudge(&earlier, Some(OFFSET_STEP_MS));
        if let Some(reset) = later.next_sibling().and_downcast::<gtk::Button>() {
            nudge(&reset, None);
        }
        // Scrolling by hand pauses following the song for a moment.
        let scroll = gtk::EventControllerScroll::builder()
            .flags(gtk::EventControllerScrollFlags::VERTICAL)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        scroll.connect_scroll({
            let view = Rc::downgrade(&view);
            move |_, _, _| {
                if let Some(view) = view.upgrade() {
                    view.user_scrolled.set(Some(Instant::now()));
                    if let Some(animation) = view.animation.take() {
                        animation.skip();
                    }
                }
                glib::Propagation::Proceed
            }
        });
        view.scrolled.add_controller(scroll);

        self.lyrics.replace(Some(view.clone()));
        view
    }

    /// Fetches the lyrics for the song in the player bar, if the Lyrics
    /// page exists and shows another song's.
    pub(super) fn load_lyrics(self: &Rc<Self>) {
        let Some(view) = self.lyrics.borrow().clone() else { return };
        let Some(track) = self.shown_track() else { return };
        if view.uri.borrow().as_deref() == Some(track.uri.as_str()) {
            return;
        }
        view.uri.replace(Some(track.uri.clone()));
        view.offset_ms.set(0);
        view.show_offset();
        view.lines.borrow_mut().clear();
        view.current.set(None);
        view.stack.set_visible_child_name("loading");
        let app = self.clone();
        glib::spawn_future_local(async move {
            let backend = app.backend.clone();
            let uri = track.uri.clone();
            let result = tokio(async move { backend.lyrics(track).await }).await;
            if view.uri.borrow().as_deref() != Some(uri.as_str()) {
                return;
            }
            match result {
                Ok(Some(lyrics)) => app.show_lyrics(&view, lyrics),
                Ok(None) => show_message(&view, "audio-x-generic-symbolic", "No Lyrics for This Song", "LRCLIB, the open lyrics database Tune uses, doesn't have them yet."),
                Err(e) => {
                    // Let the next attempt (reopening the page) try again.
                    view.uri.replace(None);
                    show_message(&view, "network-offline-symbolic", "Couldn't Load Lyrics", &e);
                }
            }
        });
    }

    fn show_lyrics(self: &Rc<Self>, view: &Rc<LyricsView>, lyrics: Lyrics) {
        if lyrics.instrumental {
            show_message(view, "audio-x-generic-symbolic", "Instrumental", "This song has no words. Enjoy the music.");
            return;
        }
        while let Some(child) = view.lines_box.first_child() {
            view.lines_box.remove(&child);
        }
        let mut lines = Vec::new();
        if lyrics.synced.is_empty() {
            for text in lyrics.plain.lines() {
                let label = line_label(text);
                label.add_css_class("plain");
                view.lines_box.append(&label);
            }
        } else {
            for (start_ms, text) in lyrics.synced {
                let button = gtk::Button::builder().child(&line_label(&text)).css_classes(["flat", "lyric-line"]).build();
                button.connect_clicked({
                    let app = Rc::downgrade(self);
                    let view = Rc::downgrade(view);
                    move |_| {
                        let (Some(app), Some(view)) = (app.upgrade(), view.upgrade()) else { return };
                        view.user_scrolled.set(None);
                        let target = (i64::from(start_ms) - i64::from(view.offset_ms.get())).max(0);
                        app.seek_to(target as u32);
                    }
                });
                view.lines_box.append(&button);
                lines.push((start_ms, button.upcast()));
            }
        }
        view.lines.replace(lines);
        view.current.set(None);
        view.user_scrolled.set(None);
        view.scrolled.vadjustment().set_value(0.0);
        view.stack.set_visible_child_name("lines");
        self.lyrics_tick(self.clock.get().now_ms());
    }

    /// Lights the line being sung and scrolls it into place.
    pub(super) fn lyrics_tick(&self, position_ms: u32) {
        let Some(view) = self.lyrics.borrow().clone() else { return };
        if self.nav.visible_page().as_ref() != Some(&view.page) {
            return;
        }
        let lines = view.lines.borrow();
        if lines.is_empty() {
            return;
        }
        let at = i64::from(position_ms) + i64::from(view.offset_ms.get());
        let index = lines.partition_point(|(start, _)| i64::from(*start) <= at).checked_sub(1);
        if index == view.current.get() {
            return;
        }
        view.current.set(index);
        for (i, (_, widget)) in lines.iter().enumerate() {
            let (past, current) = match index {
                Some(index) => (i < index, i == index),
                None => (false, false),
            };
            set_class(widget, "past", past);
            set_class(widget, "current", current);
        }
        let paused = view.user_scrolled.get().is_some_and(|at| at.elapsed() < USER_SCROLL_PAUSE);
        if let Some(index) = index
            && !paused
        {
            scroll_to(&view, &lines[index].1);
        }
    }
}

fn line_label(text: &str) -> gtk::Label {
    // Instrumental breaks are empty lines; show a note instead of nothing.
    let text = if text.trim().is_empty() { "♪" } else { text };
    gtk::Label::builder().label(text).xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).css_classes(["lyric-text"]).build()
}

fn set_class(widget: &gtk::Widget, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}

fn show_message(view: &LyricsView, icon: &str, title: &str, description: &str) {
    view.lines.borrow_mut().clear();
    view.message.set_icon_name(Some(icon));
    view.message.set_title(title);
    view.message.set_description(Some(&glib::markup_escape_text(description)));
    view.stack.set_visible_child_name("message");
}

/// Glides the view so `line` sits a third of the way down.
fn scroll_to(view: &LyricsView, line: &gtk::Widget) {
    let adjustment = view.scrolled.vadjustment();
    // Measured from the top of the lyrics, not the visible area, which
    // moves with the scrolling.
    let Some(content) = view.lines_box.parent().and_then(|clamp| clamp.parent()) else { return };
    let Some(point) = line.compute_point(&content, &gtk::graphene::Point::new(0.0, 0.0)) else { return };
    let target = (f64::from(point.y()) - adjustment.page_size() * CURRENT_LINE_AT).clamp(0.0, (adjustment.upper() - adjustment.page_size()).max(0.0));
    if let Some(animation) = view.animation.take() {
        animation.skip();
    }
    let animation = adw::TimedAnimation::builder()
        .widget(&view.scrolled)
        .value_from(adjustment.value())
        .value_to(target)
        .duration(450)
        .easing(adw::Easing::EaseOutCubic)
        .target(&adw::PropertyAnimationTarget::new(&adjustment, "value"))
        .build();
    animation.play();
    view.animation.replace(Some(animation));
}

fn find_header(widget: &gtk::Widget) -> Option<adw::HeaderBar> {
    if let Some(header) = widget.downcast_ref::<adw::HeaderBar>() {
        return Some(header.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = find_header(&current) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}
