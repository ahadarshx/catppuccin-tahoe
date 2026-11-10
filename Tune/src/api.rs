//! The Spotify Web API calls Tune needs: who is logged in, their library,
//! albums, artists, search, and saving songs. Everything is reduced to the
//! small structs below, which is all the interface shows.
//!
//! Spotify's February 2026 changes apply: search pages hold 10 results,
//! playlists list "items", the library is saved through /me/library, and
//! artist top tracks and new releases are gone from the Web API.

use std::time::Duration;

use serde_json::{Value, json};

const BASE: &str = "https://api.spotify.com/v1";
/// Upper bound on tracks fetched for one list, so a huge playlist stays quick.
const MAX_TRACKS: usize = 1000;
/// /me/library takes at most this many URIs per call.
const LIBRARY_BATCH: usize = 40;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ArtistRef {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Track {
    pub uri: String,
    pub name: String,
    /// The artists' names, joined for display.
    pub artists: String,
    pub album: String,
    pub duration_ms: u32,
    /// About 64px, for list rows and the player bar.
    pub image_small: Option<String>,
    /// About 300px, for cards, the Now Playing view and MPRIS.
    pub image_large: Option<String>,
    #[serde(default)]
    pub album_id: Option<String>,
    #[serde(default)]
    pub artist_refs: Vec<ArtistRef>,
    /// Position on its album, from 1; 0 when unknown.
    #[serde(default)]
    pub track_number: u32,
    /// When it was added to the playlist or library being shown (RFC 3339).
    #[serde(default)]
    pub added_at: Option<String>,
    /// Its place in the playlist being shown, counting entries Tune hides.
    #[serde(default)]
    pub position: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub image: Option<String>,
    pub owner_id: String,
    pub collaborative: bool,
}

#[derive(Clone, Debug)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artists: String,
    pub image: Option<String>,
    /// "album", "single" or "compilation".
    pub kind: String,
    pub year: String,
}

#[derive(Clone, Debug)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub image: Option<String>,
}

/// What a page header shows above a list of songs.
#[derive(Clone, Debug, Default)]
pub struct Header {
    /// "Album", "Single", "Playlist"…
    pub kind: String,
    pub title: String,
    /// Owner or artists, and the year for albums.
    pub byline: Vec<String>,
    pub description: String,
    pub image: Option<String>,
    pub artist_refs: Vec<ArtistRef>,
}

#[derive(Default)]
pub struct SearchResults {
    pub tracks: Vec<Track>,
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub playlists: Vec<Playlist>,
}

#[derive(Debug)]
pub enum ApiError {
    /// The token was refused; logging in again fixes it.
    Unauthorized,
    /// The login lacks a permission this call needs.
    Forbidden(String),
    Http(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => write!(f, "Spotify refused the login"),
            Self::Forbidden(message) | Self::Http(message) => write!(f, "{message}"),
        }
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.to_string())
    }
}

pub async fn get(http: &reqwest::Client, token: &str, url: &str) -> Result<Value, ApiError> {
    send(http, token, reqwest::Method::GET, url, None).await
}

/// One call; an empty reply comes back as `Value::Null`.
async fn send(http: &reqwest::Client, token: &str, method: reqwest::Method, url: &str, body: Option<&Value>) -> Result<Value, ApiError> {
    // One retry when rate limited: Spotify says how long to wait.
    for attempt in 0..2 {
        let mut request = http.request(method.clone(), url).bearer_auth(token);
        request = match body {
            Some(body) => request.json(body),
            None if method != reqwest::Method::GET => request.header(reqwest::header::CONTENT_LENGTH, 0),
            None => request,
        };
        let response = request.send().await?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait: u64 = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok())
                .unwrap_or(2);
            log::warn!("Web API rate limited on {url}; Spotify asks to wait {wait}s");
            // Short waits are worth sitting out once; long ones are reported.
            if attempt == 0 && wait <= 10 {
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            return Err(ApiError::Http(format!(
                "Spotify is rate limiting Tune; it asks to wait {}",
                human_duration(wait)
            )));
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized);
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let message = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|json| json["error"]["message"].as_str().map(str::to_owned))
                .unwrap_or_else(|| status.to_string());
            let message = format!("Spotify: {message}");
            return Err(if status == reqwest::StatusCode::FORBIDDEN { ApiError::Forbidden(message) } else { ApiError::Http(message) });
        }
        let bytes = response.bytes().await?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        return serde_json::from_slice(&bytes).map_err(|e| ApiError::Http(e.to_string()));
    }
    Err(ApiError::Http("Spotify is rate limiting requests; try again shortly".into()))
}

fn human_duration(seconds: u64) -> String {
    match seconds {
        0..=90 => format!("{seconds} seconds"),
        91..=5400 => format!("{} minutes", seconds.div_ceil(60)),
        _ => format!("{} hours", seconds.div_ceil(3600)),
    }
}

fn url_with(path: &str, params: &[(&str, &str)]) -> Result<String, ApiError> {
    reqwest::Url::parse_with_params(&format!("{BASE}{path}"), params)
        .map(String::from)
        .map_err(|e| ApiError::Http(e.to_string()))
}

/// The user's ID and display name.
pub async fn me(http: &reqwest::Client, token: &str) -> Result<(String, String), ApiError> {
    let me = get(http, token, &format!("{BASE}/me")).await?;
    let id = me["id"].as_str().unwrap_or_default().to_owned();
    let name = me["display_name"].as_str().or(me["id"].as_str()).unwrap_or("Spotify user").to_owned();
    Ok((id, name))
}

pub async fn playlists(http: &reqwest::Client, token: &str) -> Result<Vec<Playlist>, ApiError> {
    let mut playlists = Vec::new();
    let mut url = Some(format!("{BASE}/me/playlists?limit=50"));
    while let Some(next) = url {
        let page = get(http, token, &next).await?;
        playlists.extend(page["items"].as_array().into_iter().flatten().filter_map(|item| parse_playlist(item, 64)));
        url = page["next"].as_str().map(str::to_owned);
    }
    Ok(playlists)
}

/// Part of a long list of songs, and where the rest continues.
pub struct Page {
    pub tracks: Vec<Track>,
    pub next: Option<String>,
    /// How many songs the whole list has, when Spotify says.
    pub total: Option<usize>,
}

pub async fn liked_page(http: &reqwest::Client, token: &str) -> Result<Page, ApiError> {
    track_page(http, token, &format!("{BASE}/me/tracks?limit=50")).await
}

pub async fn playlist_page(http: &reqwest::Client, token: &str, id: &str) -> Result<Page, ApiError> {
    // Spotify has been renaming this endpoint from /tracks to /items; accept either.
    match track_page(http, token, &format!("{BASE}/playlists/{id}/items?limit=100")).await {
        Err(ApiError::Http(_)) => track_page(http, token, &format!("{BASE}/playlists/{id}/tracks?limit=100")).await,
        result => result,
    }
}

/// One page of saved tracks or playlist entries, which wrap the track; newer
/// playlist responses call it "item".
pub async fn track_page(http: &reqwest::Client, token: &str, url: &str) -> Result<Page, ApiError> {
    let page = get(http, token, url).await?;
    let offset = page["offset"].as_u64().unwrap_or(0) as usize;
    let tracks = page["items"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, item)| {
            let mut track = parse_track(if item["track"].is_object() { &item["track"] } else { &item["item"] })?;
            track.added_at = item["added_at"].as_str().map(str::to_owned);
            track.position = Some(offset + index);
            Some(track)
        })
        .collect();
    Ok(Page { tracks, next: page["next"].as_str().map(str::to_owned), total: page["total"].as_u64().map(|n| n as usize) })
}

pub async fn playlist_header(http: &reqwest::Client, token: &str, id: &str) -> Result<Header, ApiError> {
    let url = url_with(&format!("/playlists/{id}"), &[("fields", "name,description,owner(id,display_name),images")])?;
    let playlist = get(http, token, &url).await?;
    let owner = playlist["owner"]["display_name"].as_str().or(playlist["owner"]["id"].as_str()).unwrap_or_default();
    Ok(Header {
        kind: "Playlist".into(),
        title: playlist["name"].as_str().unwrap_or_default().to_owned(),
        byline: vec![owner.to_owned()].into_iter().filter(|s| !s.is_empty()).collect(),
        // Descriptions come with HTML entities and links.
        description: strip_html(playlist["description"].as_str().unwrap_or_default()),
        image: pick_image(&playlist["images"], 300),
        artist_refs: Vec::new(),
    })
}

fn strip_html(text: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&#x27;", "'").replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">")
}

/// Search pages hold 10 results; songs get three pages, the rest one.
pub async fn search(http: &reqwest::Client, token: &str, query: &str) -> Result<SearchResults, ApiError> {
    const PAGE: usize = 10;
    let url = url_with("/search", &[("q", query), ("type", "track,artist,album,playlist"), ("limit", "10")])?;
    let first = get(http, token, &url).await?;
    let items = |kind: &str| first[kind]["items"].as_array().cloned().unwrap_or_default();
    let mut results = SearchResults {
        tracks: items("tracks").iter().filter_map(parse_track).collect(),
        artists: items("artists").iter().filter_map(parse_artist).collect(),
        albums: items("albums").iter().filter_map(parse_album).collect(),
        playlists: items("playlists").iter().filter_map(|p| parse_playlist(p, 300)).collect(),
    };
    if results.tracks.len() < PAGE {
        return Ok(results);
    }
    for offset in [PAGE, 2 * PAGE] {
        let offset = offset.to_string();
        let url = url_with("/search", &[("q", query), ("type", "track"), ("limit", "10"), ("offset", offset.as_str())])?;
        let Ok(page) = get(http, token, &url).await else { break };
        let more: Vec<Track> = page["tracks"]["items"].as_array().into_iter().flatten().filter_map(parse_track).collect();
        let last = more.len() < PAGE;
        results.tracks.extend(more);
        if last {
            break;
        }
    }
    Ok(results)
}

// Home

pub async fn recently_played(http: &reqwest::Client, token: &str) -> Result<Vec<Track>, ApiError> {
    let page = get(http, token, &format!("{BASE}/me/player/recently-played?limit=50")).await?;
    let mut seen = std::collections::HashSet::new();
    Ok(page["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| parse_track(&item["track"]))
        .filter(|track| seen.insert(track.uri.clone()))
        .collect())
}

pub async fn top_tracks(http: &reqwest::Client, token: &str) -> Result<Vec<Track>, ApiError> {
    let page = get(http, token, &format!("{BASE}/me/top/tracks?limit=20&time_range=short_term")).await?;
    Ok(page["items"].as_array().into_iter().flatten().filter_map(parse_track).collect())
}

pub async fn top_artists(http: &reqwest::Client, token: &str) -> Result<Vec<Artist>, ApiError> {
    let page = get(http, token, &format!("{BASE}/me/top/artists?limit=12&time_range=medium_term")).await?;
    Ok(page["items"].as_array().into_iter().flatten().filter_map(parse_artist).collect())
}

pub async fn saved_albums(http: &reqwest::Client, token: &str) -> Result<Vec<Album>, ApiError> {
    let page = get(http, token, &format!("{BASE}/me/albums?limit=20")).await?;
    Ok(page["items"].as_array().into_iter().flatten().filter_map(|item| parse_album(&item["album"])).collect())
}

// Album and artist pages

pub async fn album(http: &reqwest::Client, token: &str, id: &str) -> Result<(Header, Vec<Track>), ApiError> {
    let album = get(http, token, &format!("{BASE}/albums/{id}")).await?;
    // Tracks listed under an album leave the album out; it is the parent.
    let mut tracks = Vec::new();
    let mut page = album["tracks"].clone();
    loop {
        for item in page["items"].as_array().into_iter().flatten() {
            let mut item = item.clone();
            item["album"] = album.clone();
            if let Some(track) = parse_track(&item) {
                tracks.push(track);
            }
        }
        match page["next"].as_str() {
            Some(next) if tracks.len() < MAX_TRACKS => page = get(http, token, next).await?,
            _ => break,
        }
    }
    let year = album["release_date"].as_str().unwrap_or_default().get(..4).unwrap_or_default().to_owned();
    let mut byline = vec![join_names(&album["artists"])];
    if !year.is_empty() {
        byline.push(year);
    }
    let header = Header {
        kind: capitalize(album["album_type"].as_str().unwrap_or("album")),
        title: album["name"].as_str().unwrap_or_default().to_owned(),
        byline,
        description: album["label"].as_str().map(|label| format!("℗ {label}")).unwrap_or_default(),
        image: pick_image(&album["images"], 300),
        artist_refs: artist_refs(&album["artists"]),
    };
    Ok((header, tracks))
}

pub async fn artist_albums(http: &reqwest::Client, token: &str, id: &str) -> Result<Vec<Album>, ApiError> {
    let url = url_with(&format!("/artists/{id}/albums"), &[("include_groups", "album,single"), ("limit", "10")])?;
    let page = get(http, token, &url).await?;
    Ok(page["items"].as_array().into_iter().flatten().filter_map(parse_album).collect())
}

pub async fn artist(http: &reqwest::Client, token: &str, id: &str) -> Result<Artist, ApiError> {
    let artist = get(http, token, &format!("{BASE}/artists/{id}")).await?;
    parse_artist(&artist).ok_or_else(|| ApiError::Http("Spotify sent no artist".into()))
}

// Saving

/// Whether each URI is saved in the library, in order.
pub async fn library_contains(http: &reqwest::Client, token: &str, uris: &[String]) -> Result<Vec<bool>, ApiError> {
    let mut saved = Vec::new();
    for batch in uris.chunks(LIBRARY_BATCH) {
        let url = url_with("/me/library/contains", &[("uris", &batch.join(","))])?;
        let reply = get(http, token, &url).await?;
        saved.extend(reply.as_array().into_iter().flatten().map(|v| v.as_bool().unwrap_or(false)));
    }
    Ok(saved)
}

pub async fn set_saved(http: &reqwest::Client, token: &str, uris: &[String], saved: bool) -> Result<(), ApiError> {
    let method = if saved { reqwest::Method::PUT } else { reqwest::Method::DELETE };
    for batch in uris.chunks(LIBRARY_BATCH) {
        let url = url_with("/me/library", &[("uris", &batch.join(","))])?;
        send(http, token, method.clone(), &url, None).await?;
    }
    Ok(())
}

pub async fn add_to_playlist(http: &reqwest::Client, token: &str, id: &str, uris: &[String]) -> Result<(), ApiError> {
    send(http, token, reqwest::Method::POST, &format!("{BASE}/playlists/{id}/items"), Some(&json!({ "uris": uris }))).await?;
    Ok(())
}

pub async fn create_playlist(http: &reqwest::Client, token: &str, name: &str) -> Result<Playlist, ApiError> {
    let body = json!({ "name": name, "public": false });
    let reply = send(http, token, reqwest::Method::POST, &format!("{BASE}/me/playlists"), Some(&body)).await?;
    parse_playlist(&reply, 64).ok_or_else(|| ApiError::Http("Spotify sent back an unexpected playlist".into()))
}

/// Changes a playlist's name, and its description when one is given.
pub async fn update_playlist(
    http: &reqwest::Client,
    token: &str,
    id: &str,
    name: &str,
    description: Option<&str>,
) -> Result<(), ApiError> {
    let mut body = json!({ "name": name });
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    send(http, token, reqwest::Method::PUT, &format!("{BASE}/playlists/{id}"), Some(&body)).await?;
    Ok(())
}

/// Removes every copy of a song from a playlist.
pub async fn remove_from_playlist(http: &reqwest::Client, token: &str, id: &str, uri: &str) -> Result<(), ApiError> {
    let body = json!({ "items": [{ "uri": uri }] });
    send(http, token, reqwest::Method::DELETE, &format!("{BASE}/playlists/{id}/items"), Some(&body)).await?;
    Ok(())
}

/// Moves the entry at `from` to just before the entry at `before`.
pub async fn move_in_playlist(http: &reqwest::Client, token: &str, id: &str, from: usize, before: usize) -> Result<(), ApiError> {
    let body = json!({ "range_start": from, "insert_before": before });
    send(http, token, reqwest::Method::PUT, &format!("{BASE}/playlists/{id}/items"), Some(&body)).await?;
    Ok(())
}

// Parsing

pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn parse_playlist(item: &Value, image_size: u64) -> Option<Playlist> {
    Some(Playlist {
        id: item["id"].as_str()?.to_owned(),
        name: item["name"].as_str()?.to_owned(),
        image: pick_image(&item["images"], image_size),
        owner_id: item["owner"]["id"].as_str().unwrap_or_default().to_owned(),
        collaborative: item["collaborative"].as_bool().unwrap_or(false),
    })
}

fn parse_album(album: &Value) -> Option<Album> {
    Some(Album {
        id: album["id"].as_str()?.to_owned(),
        name: album["name"].as_str()?.to_owned(),
        artists: join_names(&album["artists"]),
        image: pick_image(&album["images"], 300),
        kind: album["album_type"].as_str().unwrap_or("album").to_owned(),
        year: album["release_date"].as_str().unwrap_or_default().get(..4).unwrap_or_default().to_owned(),
    })
}

fn parse_artist(artist: &Value) -> Option<Artist> {
    Some(Artist {
        id: artist["id"].as_str()?.to_owned(),
        name: artist["name"].as_str()?.to_owned(),
        image: pick_image(&artist["images"], 300),
    })
}

fn join_names(list: &Value) -> String {
    list.as_array()
        .map(|items| items.iter().filter_map(|a| a["name"].as_str()).collect::<Vec<_>>().join(", "))
        .unwrap_or_default()
}

fn artist_refs(list: &Value) -> Vec<ArtistRef> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| Some(ArtistRef { id: a["id"].as_str()?.to_owned(), name: a["name"].as_str()?.to_owned() }))
        .collect()
}

fn parse_track(track: &Value) -> Option<Track> {
    // Podcast episodes and local files cannot be streamed by Tune.
    let uri = track["uri"].as_str()?;
    if !uri.starts_with("spotify:track:") || track["is_local"].as_bool() == Some(true) {
        return None;
    }
    let album = &track["album"];
    Some(Track {
        uri: uri.to_owned(),
        name: track["name"].as_str().unwrap_or("Unknown").to_owned(),
        artists: join_names(&track["artists"]),
        album: album["name"].as_str().unwrap_or_default().to_owned(),
        duration_ms: track["duration_ms"].as_u64().unwrap_or(0) as u32,
        image_small: pick_image(&album["images"], 64),
        image_large: pick_image(&album["images"], 300),
        album_id: album["id"].as_str().map(str::to_owned),
        artist_refs: artist_refs(&track["artists"]),
        track_number: track["track_number"].as_u64().unwrap_or(0) as u32,
        added_at: None,
        position: None,
    })
}

/// The smallest image at least `size` pixels wide, or the largest there is.
fn pick_image(images: &Value, size: u64) -> Option<String> {
    let mut images: Vec<(u64, &str)> = images
        .as_array()?
        .iter()
        .filter_map(|image| Some((image["width"].as_u64().unwrap_or(0), image["url"].as_str()?)))
        .collect();
    images.sort_by_key(|(width, _)| *width);
    images
        .iter()
        .find(|(width, _)| *width >= size)
        .or(images.last())
        .map(|(_, url)| (*url).to_owned())
}
