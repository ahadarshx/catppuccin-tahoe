//! Changing playlists: making, renaming and deleting them, and removing and
//! reordering their songs.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::App;
use super::songs::{Entry, SongRow, entry};
use crate::api::Track;
use crate::backend::TrackSource;
use crate::tokio;

/// A playlist the user may change, as shown in a song list.
pub struct PlaylistEditor {
    pub id: String,
    pub tracks: Rc<RefCell<Vec<Track>>>,
    pub store: gio::ListStore,
    /// False while songs are still arriving or a change is on its way:
    /// positions are only right once the list is whole and settled.
    pub ready: Cell<bool>,
}

impl PlaylistEditor {
    /// Redraws `count` song entries starting at song `first`.
    fn redraw(&self, first: usize, count: usize, new_count: usize) {
        let songs_start = (0..self.store.n_items()).find(|&i| {
            self.store.item(i).is_some_and(|object| matches!(entry(&object).as_deref(), Some(Entry::Song(_))))
        });
        let Some(start) = songs_start else { return };
        let fresh: Vec<glib::Object> =
            (first..first + new_count).map(|i| glib::BoxedAnyObject::new(Entry::Song(i)).upcast()).collect();
        self.store.splice(start + first as u32, count as u32, &fresh);
    }
}

/// What Spotify needs to move the song at index `from` to index `to`:
/// its position, and the position it goes in front of.
fn plan_move(tracks: &[Track], from: usize, to: usize) -> Option<(usize, usize)> {
    if from == to {
        return None;
    }
    let start = tracks.get(from)?.position?;
    let target = tracks.get(to)?.position?;
    Some((start, if to > from { target + 1 } else { target }))
}

/// Does locally what Spotify did. Positions count entries Tune hides, such
/// as podcasts, so they are shifted rather than renumbered.
fn apply_move(tracks: &mut Vec<Track>, from: usize, to: usize) {
    let Some((start, before)) = plan_move(tracks, from, to) else { return };
    for track in tracks.iter_mut() {
        let Some(position) = track.position.as_mut() else { continue };
        if *position == start {
            *position = if before > start { before - 1 } else { before };
        } else if before > start && *position > start && *position < before {
            *position -= 1;
        } else if before < start && *position >= before && *position < start {
            *position += 1;
        }
    }
    let track = tracks.remove(from);
    tracks.insert(to, track);
}

/// Removes every copy of a song, as Spotify does, and returns the index of
/// the first one.
fn apply_removal(tracks: &mut Vec<Track>, uri: &str) -> Option<usize> {
    let first = tracks.iter().position(|track| track.uri == uri)?;
    let removed: Vec<usize> = tracks.iter().filter(|t| t.uri == uri).filter_map(|t| t.position).collect();
    tracks.retain(|track| track.uri != uri);
    for position in tracks.iter_mut().filter_map(|track| track.position.as_mut()) {
        *position -= removed.iter().filter(|&&gone| gone < *position).count();
    }
    Some(first)
}

/// The index a song dragged from `from` ends up at, dropped on the top or
/// bottom half of the row at `row`.
fn drop_index(from: usize, row: usize, below: bool) -> usize {
    let slot = if below { row + 1 } else { row };
    if from < slot { slot - 1 } else { slot }
}

fn name_row(title: &str, text: &str) -> adw::EntryRow {
    let row = adw::EntryRow::builder().title(title).text(text).activates_default(true).build();
    row.set_size_request(320, -1);
    row
}

fn boxed_list(rows: &[&adw::EntryRow]) -> gtk::ListBox {
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
    for row in rows {
        list.append(*row);
    }
    list
}

impl App {
    /// Asks for a name and makes a playlist, with `track` in it if given.
    pub(super) fn ask_new_playlist(self: &Rc<Self>, track: Option<Track>) {
        let name = name_row("Name", "");
        let dialog = adw::AlertDialog::new(Some("New Playlist"), None);
        dialog.add_responses(&[("cancel", "Cancel"), ("create", "Create")]);
        dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("create"));
        dialog.set_close_response("cancel");
        dialog.set_response_enabled("create", false);
        dialog.set_extra_child(Some(&boxed_list(&[&name])));
        name.connect_changed({
            let dialog = dialog.clone();
            move |row| dialog.set_response_enabled("create", !row.text().trim().is_empty())
        });
        dialog.connect_response(Some("create"), {
            let app = self.clone();
            let name = name.clone();
            move |_, _| {
                let name = name.text().trim().to_owned();
                if !name.is_empty() {
                    glib::spawn_future_local(app.clone().create_playlist(name, track.clone()));
                }
            }
        });
        dialog.present(Some(&self.window));
        name.grab_focus();
    }

    async fn create_playlist(self: Rc<Self>, name: String, track: Option<Track>) {
        let backend = self.backend.clone();
        let request = name.clone();
        let playlist = match tokio(async move { backend.create_playlist(request).await }).await {
            Ok(playlist) => playlist,
            Err(e) => return self.toast(&e),
        };
        if let Some(track) = track {
            let backend = self.backend.clone();
            let (id, uri) = (playlist.id.clone(), track.uri);
            if let Err(e) = tokio(async move { backend.add_to_playlist(id, uri).await }).await {
                self.toast(&e);
            }
        }
        self.load_sidebar().await;
        self.select_playlist(&playlist.id);
        self.open_tracks(&name, TrackSource::Playlist(playlist.id), false);
    }

    fn select_playlist(&self, id: &str) {
        let index = self.sidebar.items.borrow().iter().position(|item| {
            matches!(item, super::SidebarItem::Playlist(playlist) if playlist.id == id)
        });
        if let Some(index) = index {
            self.sidebar.list.select_row(self.sidebar.list.row_at_index(index as i32).as_ref());
        }
    }

    /// The menu on a page of the user's own playlist.
    pub(super) fn playlist_menu(
        self: &Rc<Self>,
        id: &str,
        title: &gtk::Label,
        description: &gtk::Label,
        window_title: &adw::WindowTitle,
    ) -> gtk::MenuButton {
        let group = gio::SimpleActionGroup::new();
        let edit = gio::SimpleAction::new("edit", None);
        edit.connect_activate({
            let app = Rc::downgrade(self);
            let id = id.to_owned();
            let (title, description, window_title) = (title.clone(), description.clone(), window_title.clone());
            move |_, _| {
                if let Some(app) = app.upgrade() {
                    app.ask_edit_playlist(id.clone(), &title, &description, &window_title);
                }
            }
        });
        group.add_action(&edit);
        let delete = gio::SimpleAction::new("delete", None);
        delete.connect_activate({
            let app = Rc::downgrade(self);
            let id = id.to_owned();
            let title = title.clone();
            move |_, _| {
                if let Some(app) = app.upgrade() {
                    app.ask_delete_playlist(id.clone(), title.label().to_string());
                }
            }
        });
        group.add_action(&delete);

        let menu = gio::Menu::new();
        menu.append(Some("Edit Details…"), Some("playlist.edit"));
        menu.append(Some("Delete Playlist…"), Some("playlist.delete"));
        let button = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("More")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .menu_model(&menu)
            .build();
        button.insert_action_group("playlist", Some(&group));
        button
    }

    fn ask_edit_playlist(
        self: &Rc<Self>,
        id: String,
        title: &gtk::Label,
        description: &gtk::Label,
        window_title: &adw::WindowTitle,
    ) {
        let old_description = description.label().to_string();
        let name = name_row("Name", &title.label());
        let about = name_row("Description", &old_description);
        let dialog = adw::AlertDialog::new(Some("Edit Details"), None);
        dialog.add_responses(&[("cancel", "Cancel"), ("save", "Save")]);
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");
        dialog.set_extra_child(Some(&boxed_list(&[&name, &about])));
        name.connect_changed({
            let dialog = dialog.clone();
            move |row| dialog.set_response_enabled("save", !row.text().trim().is_empty())
        });
        dialog.connect_response(Some("save"), {
            let app = self.clone();
            let (name, about) = (name.clone(), about.clone());
            let (title, description, window_title) = (title.clone(), description.clone(), window_title.clone());
            move |_, _| {
                let new_name = name.text().trim().to_owned();
                let new_about = about.text().trim().to_owned();
                let changed = (new_about != old_description).then(|| new_about.clone());
                let app = app.clone();
                let id = id.clone();
                let (title, description, window_title) = (title.clone(), description.clone(), window_title.clone());
                glib::spawn_future_local(async move {
                    let backend = app.backend.clone();
                    let request = new_name.clone();
                    match tokio(async move { backend.update_playlist(id, request, changed).await }).await {
                        Ok(()) => {
                            title.set_label(&new_name);
                            window_title.set_title(&new_name);
                            description.set_label(&new_about);
                            description.set_visible(!new_about.is_empty());
                            app.load_sidebar().await;
                        }
                        Err(e) => app.toast(&e),
                    }
                });
            }
        });
        dialog.present(Some(&self.window));
    }

    fn ask_delete_playlist(self: &Rc<Self>, id: String, name: String) {
        let dialog = adw::AlertDialog::new(
            Some("Delete Playlist?"),
            Some(&format!("“{name}” will be removed from your Spotify library.")),
        );
        dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.connect_response(Some("delete"), {
            let app = self.clone();
            move |_, _| {
                let app = app.clone();
                let id = id.clone();
                let name = name.clone();
                glib::spawn_future_local(async move {
                    let backend = app.backend.clone();
                    match tokio(async move { backend.delete_playlist(id).await }).await {
                        Ok(()) => {
                            app.load_sidebar().await;
                            app.sidebar.list.select_row(app.sidebar.list.row_at_index(0).as_ref());
                            app.open_sidebar_item(0);
                            app.toast(&format!("Deleted {name}"));
                        }
                        Err(e) => app.toast(&e),
                    }
                });
            }
        });
        dialog.present(Some(&self.window));
    }

    pub(super) fn move_song(self: &Rc<Self>, editor: Rc<PlaylistEditor>, from: usize, to: usize) {
        if !editor.ready.get() {
            return;
        }
        let Some((start, before)) = plan_move(&editor.tracks.borrow(), from, to) else { return };
        editor.ready.set(false);
        let app = self.clone();
        glib::spawn_future_local(async move {
            let backend = app.backend.clone();
            let id = editor.id.clone();
            match tokio(async move { backend.move_in_playlist(id, start, before).await }).await {
                Ok(()) => {
                    apply_move(&mut editor.tracks.borrow_mut(), from, to);
                    let (first, last) = (from.min(to), from.max(to));
                    let count = last - first + 1;
                    editor.redraw(first, count, count);
                }
                Err(e) => app.toast(&e),
            }
            editor.ready.set(true);
        });
    }

    pub(super) fn remove_song(self: &Rc<Self>, editor: Rc<PlaylistEditor>, track: Track) {
        if !editor.ready.get() {
            return;
        }
        editor.ready.set(false);
        let app = self.clone();
        glib::spawn_future_local(async move {
            let backend = app.backend.clone();
            let (id, uri) = (editor.id.clone(), track.uri.clone());
            match tokio(async move { backend.remove_from_playlist(id, uri).await }).await {
                Ok(()) => {
                    let old_count = editor.tracks.borrow().len();
                    let first = apply_removal(&mut editor.tracks.borrow_mut(), &track.uri);
                    if let Some(first) = first {
                        let new_count = editor.tracks.borrow().len();
                        editor.redraw(first, old_count - first, new_count - first);
                    }
                    app.toast(&format!("Removed {}", track.name));
                }
                Err(e) => app.toast(&e),
            }
            editor.ready.set(true);
        });
    }

    /// Lets a row of `editor`'s list be dragged onto another one.
    pub(super) fn make_reorderable(self: &Rc<Self>, row: &Rc<SongRow>, editor: &Rc<PlaylistEditor>) {
        let source = gtk::DragSource::builder().actions(gdk::DragAction::MOVE).build();
        source.connect_prepare({
            let row = Rc::downgrade(row);
            let editor = editor.clone();
            move |_, _, _| {
                let index = row.upgrade()?.index.get()?;
                editor.ready.get().then(|| gdk::ContentProvider::for_value(&(index as u32).to_value()))
            }
        });
        source.connect_drag_begin({
            let row = Rc::downgrade(row);
            move |source, _| {
                if let Some(row) = row.upgrade() {
                    source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&row.root))), 0, 0);
                }
            }
        });
        row.root.add_controller(source);

        let target = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
        target.connect_motion({
            let row = Rc::downgrade(row);
            move |_, _, y| {
                if let Some(row) = row.upgrade() {
                    let below = y > f64::from(row.root.height()) / 2.0;
                    row.root.remove_css_class(if below { "drop-above" } else { "drop-below" });
                    row.root.add_css_class(if below { "drop-below" } else { "drop-above" });
                }
                gdk::DragAction::MOVE
            }
        });
        target.connect_leave({
            let row = Rc::downgrade(row);
            move |_| {
                if let Some(row) = row.upgrade() {
                    row.root.remove_css_class("drop-above");
                    row.root.remove_css_class("drop-below");
                }
            }
        });
        target.connect_drop({
            let app = Rc::downgrade(self);
            let row = Rc::downgrade(row);
            let editor = editor.clone();
            move |_, value, _, y| {
                let (Some(app), Some(row)) = (app.upgrade(), row.upgrade()) else { return false };
                row.root.remove_css_class("drop-above");
                row.root.remove_css_class("drop-below");
                let (Ok(from), Some(index)) = (value.get::<u32>(), row.index.get()) else { return false };
                let below = y > f64::from(row.root.height()) / 2.0;
                app.move_song(editor.clone(), from as usize, drop_index(from as usize, index, below));
                true
            }
        });
        row.root.add_controller(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(uri: &str, position: usize) -> Track {
        Track {
            uri: uri.into(),
            name: uri.into(),
            artists: String::new(),
            album: String::new(),
            duration_ms: 0,
            image_small: None,
            image_large: None,
            album_id: None,
            artist_refs: Vec::new(),
            track_number: 0,
            added_at: None,
            position: Some(position),
        }
    }

    fn list(entries: &[(&str, usize)]) -> Vec<Track> {
        entries.iter().map(|(uri, position)| track(uri, *position)).collect()
    }

    fn shown(tracks: &[Track]) -> Vec<(String, usize)> {
        tracks.iter().map(|t| (t.uri.clone(), t.position.unwrap())).collect()
    }

    /// Spotify's reorder on the full playlist, hidden entries included.
    fn spotify_move(all: &mut Vec<&str>, start: usize, before: usize) {
        let item = all.remove(start);
        all.insert(if before > start { before - 1 } else { before }, item);
    }

    fn check_against_spotify(all: &[&str], visible: &[usize], from: usize, to: usize) {
        let mut tracks: Vec<Track> = visible.iter().map(|&i| track(all[i], i)).collect();
        let (start, before) = plan_move(&tracks, from, to).unwrap();
        let mut server = all.to_vec();
        spotify_move(&mut server, start, before);
        apply_move(&mut tracks, from, to);
        for t in &tracks {
            assert_eq!(server[t.position.unwrap()], t.uri, "position of {} after {from}->{to}", t.uri);
        }
        let order: Vec<&str> = server.iter().copied().filter(|u| !u.starts_with("hidden")).collect();
        assert_eq!(order, tracks.iter().map(|t| t.uri.as_str()).collect::<Vec<_>>());
    }

    #[test]
    fn moves_match_spotify_in_both_directions() {
        let all = ["a", "b", "c", "d", "e"];
        for from in 0..5 {
            for to in 0..5 {
                if from != to {
                    check_against_spotify(&all, &[0, 1, 2, 3, 4], from, to);
                }
            }
        }
    }

    #[test]
    fn moves_match_spotify_around_hidden_entries() {
        let all = ["a", "hidden1", "b", "c", "hidden2", "d", "e"];
        for from in 0..5 {
            for to in 0..5 {
                if from != to {
                    check_against_spotify(&all, &[0, 2, 3, 5, 6], from, to);
                }
            }
        }
    }

    #[test]
    fn moving_nowhere_is_no_move() {
        assert_eq!(plan_move(&list(&[("a", 0), ("b", 1)]), 1, 1), None);
    }

    #[test]
    fn removal_takes_every_copy_and_closes_the_gaps() {
        let mut tracks = list(&[("a", 0), ("b", 1), ("a", 3), ("c", 4)]);
        assert_eq!(apply_removal(&mut tracks, "a"), Some(0));
        assert_eq!(shown(&tracks), vec![("b".into(), 0), ("c".into(), 2)]);
    }

    #[test]
    fn removing_an_unknown_song_changes_nothing() {
        let mut tracks = list(&[("a", 0)]);
        assert_eq!(apply_removal(&mut tracks, "z"), None);
        assert_eq!(tracks.len(), 1);
    }

    #[test]
    fn drop_position_accounts_for_the_row_leaving() {
        assert_eq!(drop_index(0, 3, false), 2);
        assert_eq!(drop_index(0, 3, true), 3);
        assert_eq!(drop_index(5, 2, false), 2);
        assert_eq!(drop_index(5, 2, true), 3);
        assert_eq!(drop_index(2, 2, false), 2);
        assert_eq!(drop_index(2, 2, true), 2);
    }
}
