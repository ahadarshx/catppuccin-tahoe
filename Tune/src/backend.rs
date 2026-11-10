//! Everything that talks to Spotify: logging in, the audio session and
//! player (librespot), the Spotify Connect device, and Web API calls. It all
//! runs on the tokio runtime; the interface only calls these methods and
//! listens to `PlayerUpdate`s.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use librespot_connect::{ConnectConfig, Spirc};
use librespot_core::{SpotifyUri, authentication::Credentials, config::DeviceType, config::SessionConfig, session::Session};
use librespot_oauth::{OAuthClient, OAuthClientBuilder, OAuthToken};
use librespot_playback::{
    audio_backend,
    config::{AudioFormat, Bitrate, PlayerConfig},
    mixer::{Mixer, MixerConfig, softmixer::SoftMixer},
    player::{Player, PlayerEvent},
};

use crate::api::{self, Album, ApiError, Artist, ArtistRef, Header, Playlist, SearchResults, Track};
use crate::keyring;

/// Spotify's own desktop client ID, which librespot and ncspot log in with.
/// Spotify streams for it, but its Web API allowance is shared by every
/// librespot user and is nearly always used up.
const DESKTOP_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
const REDIRECT_URI: &str = "http://127.0.0.1:8898/login";
const WEB_SCOPES: &[&str] = &[
    "streaming",
    "user-read-private",
    "user-library-read",
    "user-library-modify",
    "playlist-read-private",
    "playlist-read-collaborative",
    "playlist-modify-private",
    "playlist-modify-public",
    "user-read-recently-played",
    "user-top-read",
];
const AUDIO_SCOPES: &[&str] = &["streaming"];
const LOGGED_IN_MESSAGE: &str = "You're logged in. You can close this tab and go back to Tune.";
/// Songs on an artist's page.
const ARTIST_TOP_TRACKS: usize = 10;
/// Covers kept on disk; the oldest go first.
const IMAGE_CACHE_FILES: usize = 3000;

fn config_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".config")))
}

/// The user's own Spotify app, from ~/.config/tune/client-id, if set up.
fn own_client_id() -> Option<String> {
    let id = std::fs::read_to_string(config_dir()?.join("tune/client-id")).ok()?;
    let id = id.trim();
    (id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())).then(|| id.to_owned())
}

fn image_cache_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".cache")))
        .map(|cache| cache.join("tune/images"))
}

/// The same Connect device across restarts, so the Spotify app does not
/// collect a new "Tune" every time.
fn device_id() -> String {
    use std::hash::{Hash, Hasher};
    let machine = std::fs::read_to_string("/etc/machine-id").unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (machine.trim(), "tune-connect").hash(&mut hasher);
    let a = hasher.finish();
    "tune".hash(&mut hasher);
    format!("{a:016x}{:016x}", hasher.finish())
}

/// Where a page of songs comes from.
#[derive(Debug, Clone)]
pub enum TrackSource {
    Liked,
    Playlist(String),
    Album(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Quality {
    Low,
    #[default]
    Normal,
    High,
}

/// How audio is played; changing it rebuilds the player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioSettings {
    pub quality: Quality,
    pub normalize: bool,
    pub gapless: bool,
}

impl AudioSettings {
    fn player_config(self) -> PlayerConfig {
        PlayerConfig {
            position_update_interval: None,
            bitrate: match self.quality {
                Quality::Low => Bitrate::Bitrate96,
                Quality::Normal => Bitrate::Bitrate160,
                Quality::High => Bitrate::Bitrate320,
            },
            normalisation: self.normalize,
            gapless: self.gapless,
            ..PlayerConfig::default()
        }
    }
}

/// What the player is doing, for the interface.
#[derive(Debug, Clone)]
pub enum PlayerUpdate {
    Playing { uri: String, position_ms: u32 },
    Paused { uri: String, position_ms: u32 },
    Seeked { uri: String, position_ms: u32 },
    EndOfTrack { uri: String },
    Unavailable { uri: String },
    /// From the Spotify Connect device: another app is playing through Tune.
    Connect(ConnectUpdate),
}

#[derive(Debug, Clone)]
pub enum ConnectUpdate {
    Track(Track),
    Playing { position_ms: u32 },
    Paused { position_ms: u32 },
    Seeked { position_ms: u32 },
    /// Another device took over, or playback stopped.
    Inactive,
    Volume(f64),
}

pub struct ArtistPage {
    pub artist: Artist,
    pub top_tracks: Vec<Track>,
    pub albums: Vec<Album>,
}

struct Token {
    access: String,
    refresh: String,
    expires_at: Instant,
}

/// One OAuth login: its client, its keyring entry and its current token.
struct Login {
    client: Arc<OAuthClient>,
    kind: keyring::Kind,
    token: tokio::sync::Mutex<Option<Token>>,
}

impl Login {
    fn new(client_id: &str, scopes: &[&str], kind: keyring::Kind) -> Result<Self, String> {
        let client = OAuthClientBuilder::new(client_id, REDIRECT_URI, scopes.to_vec())
            .open_in_browser()
            .with_custom_message(LOGGED_IN_MESSAGE)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client: Arc::new(client), kind, token: tokio::sync::Mutex::new(None) })
    }

    /// From the keyring; false when there is nothing usable stored.
    async fn restore(&self) -> bool {
        let Some(refresh) = keyring::load(self.kind).await else { return false };
        match self.client.refresh_token_async(&refresh).await {
            Ok(token) => {
                self.set(token, refresh).await;
                true
            }
            // Revoked or expired for good: a browser login is needed.
            Err(e) => {
                log::warn!("stored login no longer works: {e}");
                keyring::clear(self.kind).await;
                false
            }
        }
    }

    /// Spotify's login page in the browser; waits for it to come back.
    async fn browser(&self) -> Result<(), String> {
        // librespot waits for the browser with a blocking listener.
        let client = self.client.clone();
        let token = tokio::task::spawn_blocking(move || client.get_access_token())
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("Login failed: {e}"))?;
        let refresh = token.refresh_token.clone();
        self.set(token, refresh).await;
        Ok(())
    }

    async fn set(&self, token: OAuthToken, previous_refresh: String) {
        // Spotify may hand out a new refresh token; keep whichever is current.
        let refresh = if token.refresh_token.is_empty() { previous_refresh } else { token.refresh_token };
        if let Err(e) = keyring::save(self.kind, &refresh).await {
            log::warn!("could not save the login to the keyring: {e}");
        }
        *self.token.lock().await = Some(Token { access: token.access_token, refresh, expires_at: token.expires_at });
    }

    /// An access token that is good for at least another minute.
    async fn access(&self) -> Result<String, ApiError> {
        let mut guard = self.token.lock().await;
        let token = guard.as_mut().ok_or(ApiError::Unauthorized)?;
        if token.expires_at.saturating_duration_since(Instant::now()) < Duration::from_secs(60) {
            let fresh = self.client.refresh_token_async(&token.refresh).await.map_err(|_| ApiError::Unauthorized)?;
            token.access = fresh.access_token;
            token.expires_at = fresh.expires_at;
            if !fresh.refresh_token.is_empty() && fresh.refresh_token != token.refresh {
                token.refresh = fresh.refresh_token;
                let _ = keyring::save(self.kind, &token.refresh).await;
            }
        }
        Ok(token.access.clone())
    }

    async fn forget(&self) {
        *self.token.lock().await = None;
        keyring::clear(self.kind).await;
    }
}

/// The Spotify Connect device: its own session and player, so another app
/// playing through Tune never mixes with Tune's own queue.
struct Connect {
    spirc: Spirc,
    player: Arc<Player>,
}

pub struct Backend {
    /// For the Web API: the user's own app when there is one.
    web: Login,
    /// For streaming: Spotify's desktop client, when `web` is the user's own app.
    audio: Option<Login>,
    session: Mutex<Option<Session>>,
    player: Mutex<Option<Arc<Player>>>,
    connect: Mutex<Option<Connect>>,
    connect_enabled: std::sync::atomic::AtomicBool,
    audio_settings: Mutex<AudioSettings>,
    mixer: Arc<SoftMixer>,
    http: reqwest::Client,
    user_id: Mutex<String>,
    /// Album art downloads at a time, so a long list does not flood the network.
    image_permits: tokio::sync::Semaphore,
    updates: async_channel::Sender<PlayerUpdate>,
}

impl Backend {
    pub fn new(updates: async_channel::Sender<PlayerUpdate>, audio: AudioSettings, connect: bool) -> Result<Arc<Self>, String> {
        let (web, audio_login) = match own_client_id() {
            Some(id) => (
                Login::new(&id, WEB_SCOPES, keyring::Kind::Web)?,
                Some(Login::new(DESKTOP_CLIENT_ID, AUDIO_SCOPES, keyring::Kind::Audio)?),
            ),
            None => {
                log::warn!("no ~/.config/tune/client-id: using the shared desktop client, which Spotify rate limits");
                (Login::new(DESKTOP_CLIENT_ID, WEB_SCOPES, keyring::Kind::Audio)?, None)
            }
        };
        let mixer = SoftMixer::open(MixerConfig::default()).map_err(|e| e.to_string())?;
        Ok(Arc::new(Self {
            web,
            audio: audio_login,
            session: Mutex::new(None),
            player: Mutex::new(None),
            connect: Mutex::new(None),
            connect_enabled: std::sync::atomic::AtomicBool::new(connect),
            audio_settings: Mutex::new(audio),
            mixer: Arc::new(mixer),
            http: reqwest::Client::new(),
            user_id: Mutex::new(String::new()),
            image_permits: tokio::sync::Semaphore::new(8),
            updates,
        }))
    }

    // Login

    /// Logs in with what the keyring holds. Returns the user's name, or None
    /// when a browser login is needed.
    pub async fn restore(self: &Arc<Self>) -> Result<Option<String>, String> {
        let web = self.web.restore().await;
        let audio = match &self.audio {
            Some(audio) => audio.restore().await,
            None => true,
        };
        if !web || !audio {
            return Ok(None);
        }
        match self.connect_audio().await {
            Ok(()) => {
                self.start_connect_device();
                Ok(Some(self.account_name().await))
            }
            Err(e) => {
                log::warn!("{e}");
                Ok(None)
            }
        }
    }

    /// Spotify's login page in the browser, for each login that is not
    /// already working: the user's own app (browsing) and, with it, Spotify's
    /// desktop client, the only one Spotify hands audio to.
    pub async fn login(self: &Arc<Self>) -> Result<String, String> {
        if self.web.access().await.is_err() {
            self.web.browser().await?;
        }
        if let Some(audio) = &self.audio
            && audio.access().await.is_err()
        {
            audio.browser().await?;
        }
        self.connect_audio().await?;
        self.start_connect_device();
        Ok(self.account_name().await)
    }

    pub async fn logout(&self) {
        self.stop_connect_device();
        if let Some(player) = self.player.lock().unwrap().as_ref() {
            player.stop();
        }
        if let Some(session) = self.session.lock().unwrap().take() {
            session.shutdown();
        }
        self.web.forget().await;
        if let Some(audio) = &self.audio {
            audio.forget().await;
        }
    }

    async fn account_name(&self) -> String {
        // A nicety: a busy Web API must not undo a good login.
        let me = match self.web.access().await {
            Ok(token) => api::me(&self.http, &token).await,
            Err(e) => Err(e),
        };
        match me {
            Ok((id, name)) => {
                *self.user_id.lock().unwrap() = id;
                name
            }
            Err(e) => {
                log::warn!("could not read the account name: {e}");
                self.session.lock().unwrap().as_ref().map(Session::username).unwrap_or_default()
            }
        }
    }

    pub fn user_id(&self) -> String {
        self.user_id.lock().unwrap().clone()
    }

    async fn connect_audio(&self) -> Result<(), String> {
        let login = self.audio.as_ref().unwrap_or(&self.web);
        let access = login.access().await.map_err(|e| e.to_string())?;
        self.connect(&access).await
    }

    /// A fresh audio session, handed to the player (created on first use).
    async fn connect(&self, access: &str) -> Result<(), String> {
        let session = Session::new(SessionConfig::default(), None);
        session
            .connect(Credentials::with_access_token(access), false)
            .await
            .map_err(|e| format!("Could not connect to Spotify: {e}"))?;

        let mut player = self.player.lock().unwrap();
        match player.as_ref() {
            Some(player) => player.set_session(session.clone()),
            None => *player = Some(self.new_player(&session)?),
        }
        if let Some(old) = self.session.lock().unwrap().replace(session) {
            old.shutdown();
        }
        Ok(())
    }

    fn new_player(&self, session: &Session) -> Result<Arc<Player>, String> {
        let sink = audio_backend::find(Some("pulseaudio".into())).ok_or("This build has no PulseAudio output")?;
        let config = self.audio_settings.lock().unwrap().player_config();
        let player = Player::new(config, session.clone(), self.mixer.get_soft_volume(), move || sink(None, AudioFormat::default()));
        self.forward_events(&player);
        Ok(player)
    }

    /// New audio settings; the player is rebuilt with them. True when there
    /// was a player, so the interface reloads what was playing.
    pub fn set_audio_settings(&self, settings: AudioSettings) -> bool {
        *self.audio_settings.lock().unwrap() = settings;
        let Some(session) = self.session.lock().unwrap().clone() else { return false };
        let mut player = self.player.lock().unwrap();
        let Some(old) = player.take() else { return false };
        old.stop();
        match self.new_player(&session) {
            Ok(new) => {
                *player = Some(new);
                true
            }
            Err(e) => {
                log::warn!("{e}");
                *player = Some(old);
                false
            }
        }
    }

    async fn access_token(&self) -> Result<String, String> {
        self.web.access().await.map_err(|e| e.to_string())
    }

    // Spotify Connect

    pub fn set_connect_enabled(self: &Arc<Self>, enabled: bool) {
        self.connect_enabled.store(enabled, std::sync::atomic::Ordering::Relaxed);
        if enabled {
            self.start_connect_device();
        } else {
            self.stop_connect_device();
        }
    }

    /// Makes Tune show up as a speaker in other Spotify apps. Runs in the
    /// background; failures only mean Tune is not listed.
    fn start_connect_device(self: &Arc<Self>) {
        if !self.connect_enabled.load(std::sync::atomic::Ordering::Relaxed) || self.connect.lock().unwrap().is_some() {
            return;
        }
        let backend = self.clone();
        tokio::spawn(async move {
            if let Err(e) = backend.run_connect_device().await {
                log::warn!("Spotify Connect unavailable: {e}");
            }
        });
    }

    async fn run_connect_device(self: &Arc<Self>) -> Result<(), String> {
        let login = self.audio.as_ref().unwrap_or(&self.web);
        let access = login.access().await.map_err(|e| e.to_string())?;
        let session = Session::new(SessionConfig { device_id: device_id(), ..SessionConfig::default() }, None);
        let sink = audio_backend::find(Some("pulseaudio".into())).ok_or("no PulseAudio output")?;
        let config = self.audio_settings.lock().unwrap().player_config();
        let player = Player::new(config, session.clone(), self.mixer.get_soft_volume(), move || sink(None, AudioFormat::default()));
        let volume = self.mixer.volume();
        let connect_config = ConnectConfig {
            name: "Tune".into(),
            device_type: DeviceType::Computer,
            initial_volume: volume,
            ..ConnectConfig::default()
        };
        let (spirc, task) = Spirc::new(connect_config, session.clone(), Credentials::with_access_token(access), player.clone(), self.mixer.clone())
            .await
            .map_err(|e| e.to_string())?;
        self.forward_connect_events(&player, &session);
        *self.connect.lock().unwrap() = Some(Connect { spirc, player });
        log::info!("listed as a Spotify Connect device");
        task.await;
        // The session ended (network change, logout): drop the device.
        self.connect.lock().unwrap().take();
        let _ = self.updates.send(PlayerUpdate::Connect(ConnectUpdate::Inactive)).await;
        Ok(())
    }

    fn stop_connect_device(&self) {
        if let Some(connect) = self.connect.lock().unwrap().take() {
            let _ = connect.spirc.shutdown();
            connect.player.stop();
        }
    }

    /// Tune plays something itself: the Connect device lets go, so the two
    /// never play at once.
    fn release_connect(&self) {
        if let Some(connect) = self.connect.lock().unwrap().as_ref() {
            let _ = connect.spirc.disconnect(true);
        }
    }

    fn with_spirc(&self, f: impl FnOnce(&Spirc) -> Result<(), librespot_core::Error>) {
        if let Some(connect) = self.connect.lock().unwrap().as_ref()
            && let Err(e) = f(&connect.spirc)
        {
            log::warn!("Spotify Connect: {e}");
        }
    }

    pub fn connect_play_pause(&self) {
        self.with_spirc(Spirc::play_pause);
    }

    pub fn connect_next(&self) {
        self.with_spirc(Spirc::next);
    }

    pub fn connect_previous(&self) {
        self.with_spirc(Spirc::prev);
    }

    pub fn connect_seek(&self, position_ms: u32) {
        self.with_spirc(|spirc| spirc.set_position_ms(position_ms));
    }

    pub fn connect_shuffle(&self, shuffle: bool) {
        self.with_spirc(|spirc| spirc.shuffle(shuffle));
    }

    pub fn connect_repeat(&self, all: bool, one: bool) {
        self.with_spirc(|spirc| spirc.repeat(all || one).and_then(|()| spirc.repeat_track(one)));
    }

    // Library

    pub async fn playlists(&self) -> Result<Vec<Playlist>, String> {
        let token = self.access_token().await?;
        api::playlists(&self.http, &token).await.map_err(|e| e.to_string())
    }

    pub async fn search(&self, query: String) -> Result<SearchResults, String> {
        let token = self.access_token().await?;
        api::search(&self.http, &token, &query).await.map_err(|e| e.to_string())
    }

    /// The header and first page of songs from `source`; long lists continue
    /// with `next_page`, so the first songs show up right away.
    pub async fn first_page(&self, source: TrackSource) -> Result<(Option<Header>, api::Page), String> {
        let token = self.access_token().await?;
        match source {
            TrackSource::Liked => api::liked_page(&self.http, &token).await.map(|page| (None, page)).map_err(|e| e.to_string()),
            TrackSource::Playlist(id) => {
                let (header, page) = tokio::join!(api::playlist_header(&self.http, &token, &id), api::playlist_page(&self.http, &token, &id));
                let header = header.map_err(|e| log::warn!("playlist header: {e}")).ok();
                Ok((header, page.map_err(|e| e.to_string())?))
            }
            TrackSource::Album(id) => {
                let (header, tracks) = api::album(&self.http, &token, &id).await.map_err(|e| e.to_string())?;
                let total = Some(tracks.len());
                Ok((Some(header), api::Page { tracks, next: None, total }))
            }
        }
    }

    pub async fn next_page(&self, url: String) -> Result<api::Page, String> {
        let token = self.access_token().await?;
        api::track_page(&self.http, &token, &url).await.map_err(|e| e.to_string())
    }

    /// An artist's portrait, popular songs and releases. Spotify's Web API
    /// dropped top tracks, so those come from the metadata service that
    /// Spotify's own apps use.
    pub async fn artist_page(&self, id: String) -> Result<ArtistPage, String> {
        use librespot_metadata::Metadata;
        let session = self.session.lock().unwrap().clone().ok_or("Not connected")?;
        let token = self.access_token().await?;
        let uri = SpotifyUri::from_uri(&format!("spotify:artist:{id}")).map_err(|e| e.to_string())?;
        let (metadata, albums, web_artist) = tokio::join!(
            librespot_metadata::Artist::get(&session, &uri),
            api::artist_albums(&self.http, &token, &id),
            api::artist(&self.http, &token, &id),
        );
        let metadata = metadata.map_err(|e| e.to_string())?;
        let country = session.country();
        let top: Vec<String> = metadata
            .top_tracks
            .for_country(&country)
            .iter()
            .take(ARTIST_TOP_TRACKS)
            .filter_map(|uri| uri.to_uri().ok())
            .collect();
        let top_tracks = tracks_metadata(&session, top).await;
        let portrait = metadata.portrait_group.first().or(metadata.portraits.first()).and_then(|image| image.id.to_base16().ok());
        let artist = web_artist.unwrap_or_else(|_| Artist {
            id: id.clone(),
            name: metadata.name.clone(),
            image: portrait.map(|id| format!("https://i.scdn.co/image/{id}")),
        });
        Ok(ArtistPage { artist, top_tracks, albums: albums.unwrap_or_default() })
    }

    /// Spotify's song radio for each seed, interleaved: real recommendations
    /// from the engine behind "Go to song radio". This goes through the audio
    /// session (Spotify's own client), since the Web API no longer offers
    /// recommendations to new apps. Seeds and `exclude` are left out.
    pub async fn radio(&self, seeds: Vec<String>, exclude: Vec<String>, limit: usize) -> Result<Vec<Track>, String> {
        let session = self.session.lock().unwrap().clone().ok_or("Not connected")?;
        let mut stations = Vec::new();
        for seed in &seeds {
            match station(&session, seed).await {
                Ok(uris) => stations.push(uris),
                Err(e) => log::warn!("radio for {seed}: {e}"),
            }
        }

        // Round-robin across seeds, so each one shapes the mix.
        let mut skip: std::collections::HashSet<String> = seeds.into_iter().chain(exclude).collect();
        let mut picked = Vec::new();
        let longest = stations.iter().map(Vec::len).max().unwrap_or(0);
        'fill: for i in 0..longest {
            for station in &stations {
                if let Some(uri) = station.get(i)
                    && skip.insert(uri.clone())
                {
                    picked.push(uri.clone());
                    if picked.len() == limit {
                        break 'fill;
                    }
                }
            }
        }
        let tracks = tracks_metadata(&session, picked).await;
        log::info!("radio: {} recommendations from {} stations", tracks.len(), stations.len());
        Ok(tracks)
    }

    pub async fn recently_played(&self) -> Result<Vec<Track>, String> {
        let token = self.access_token().await?;
        api::recently_played(&self.http, &token).await.map_err(|e| e.to_string())
    }

    pub async fn top_tracks(&self) -> Result<Vec<Track>, String> {
        let token = self.access_token().await?;
        api::top_tracks(&self.http, &token).await.map_err(|e| e.to_string())
    }

    pub async fn top_artists(&self) -> Result<Vec<Artist>, String> {
        let token = self.access_token().await?;
        api::top_artists(&self.http, &token).await.map_err(|e| e.to_string())
    }

    pub async fn saved_albums(&self) -> Result<Vec<Album>, String> {
        let token = self.access_token().await?;
        api::saved_albums(&self.http, &token).await.map_err(|e| e.to_string())
    }

    pub async fn is_saved(&self, uri: String) -> Result<bool, String> {
        let token = self.access_token().await?;
        let saved = api::library_contains(&self.http, &token, &[uri]).await.map_err(|e| e.to_string())?;
        Ok(saved.first().copied().unwrap_or(false))
    }

    pub async fn set_saved(&self, uri: String, saved: bool) -> Result<(), String> {
        let token = self.access_token().await?;
        api::set_saved(&self.http, &token, &[uri], saved).await.map_err(permission_message)
    }

    pub async fn add_to_playlist(&self, playlist: String, uri: String) -> Result<(), String> {
        let token = self.access_token().await?;
        api::add_to_playlist(&self.http, &token, &playlist, &[uri]).await.map_err(permission_message)
    }

    pub async fn create_playlist(&self, name: String) -> Result<Playlist, String> {
        let token = self.access_token().await?;
        api::create_playlist(&self.http, &token, &name).await.map_err(permission_message)
    }

    pub async fn update_playlist(&self, id: String, name: String, description: Option<String>) -> Result<(), String> {
        let token = self.access_token().await?;
        api::update_playlist(&self.http, &token, &id, &name, description.as_deref()).await.map_err(permission_message)
    }

    /// Takes a playlist out of the user's library, which is how Spotify deletes one.
    pub async fn delete_playlist(&self, id: String) -> Result<(), String> {
        let token = self.access_token().await?;
        api::set_saved(&self.http, &token, &[format!("spotify:playlist:{id}")], false).await.map_err(permission_message)
    }

    pub async fn remove_from_playlist(&self, id: String, uri: String) -> Result<(), String> {
        let token = self.access_token().await?;
        api::remove_from_playlist(&self.http, &token, &id, &uri).await.map_err(permission_message)
    }

    pub async fn move_in_playlist(&self, id: String, from: usize, before: usize) -> Result<(), String> {
        let token = self.access_token().await?;
        api::move_in_playlist(&self.http, &token, &id, from, before).await.map_err(permission_message)
    }

    pub async fn lyrics(&self, track: Track) -> Result<Option<crate::lyrics::Lyrics>, String> {
        crate::lyrics::fetch(&self.http, &track).await
    }

    /// The bytes of an image (album art), for the interface to decode; kept
    /// on disk so covers show at once on the next start.
    pub async fn image(&self, url: String) -> Option<Vec<u8>> {
        // Spotify's image URLs end in a unique ID.
        let name: String = url.rsplit('/').next()?.chars().filter(char::is_ascii_alphanumeric).collect();
        let path = image_cache_dir().filter(|_| !name.is_empty()).map(|dir| dir.join(&name));
        if let Some(path) = &path
            && let Ok(bytes) = tokio::fs::read(path).await
        {
            return Some(bytes);
        }
        let _permit = self.image_permits.acquire().await.ok()?;
        let response = self.http.get(url).send().await.ok()?.error_for_status().ok()?;
        let bytes = response.bytes().await.ok()?.to_vec();
        if let Some(path) = path {
            let _ = tokio::fs::create_dir_all(path.parent()?).await;
            let _ = tokio::fs::write(path, &bytes).await;
        }
        Some(bytes)
    }

    /// Trims the cover cache to its size, oldest first.
    pub fn prune_image_cache() {
        let Some(dir) = image_cache_dir() else { return };
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        let mut files: Vec<_> = entries
            .flatten()
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .collect();
        if files.len() <= IMAGE_CACHE_FILES {
            return;
        }
        files.sort();
        for (_, path) in &files[..files.len() - IMAGE_CACHE_FILES] {
            let _ = std::fs::remove_file(path);
        }
    }

    // Playback

    /// Loads a song at `position_ms`; it starts right away when `play` is set.
    pub async fn load(self: &Arc<Self>, uri: String, position_ms: u32, play: bool) -> Result<(), String> {
        let id = SpotifyUri::from_uri(&uri).map_err(|e| e.to_string())?;
        // The session drops after long idle times or network changes.
        let invalid = self.session.lock().unwrap().as_ref().is_none_or(Session::is_invalid);
        if invalid {
            self.connect_audio().await?;
            self.start_connect_device();
        }
        if play {
            self.release_connect();
        }
        let player = self.player.lock().unwrap().clone().ok_or("Not logged in")?;
        player.load(id, play, position_ms);
        Ok(())
    }

    pub fn play(&self) {
        self.release_connect();
        if let Some(player) = self.player.lock().unwrap().as_ref() {
            player.play();
        }
    }

    pub fn pause(&self) {
        if let Some(player) = self.player.lock().unwrap().as_ref() {
            player.pause();
        }
    }

    pub fn seek(&self, position_ms: u32) {
        if let Some(player) = self.player.lock().unwrap().as_ref() {
            player.seek(position_ms);
        }
    }

    /// `volume` from 0 to 1; shared with the Connect device, so the other
    /// app's volume control shows it too.
    pub fn set_volume(&self, volume: f64) {
        let level = (volume.clamp(0.0, 1.0) * f64::from(u16::MAX)) as u16;
        self.mixer.set_volume(level);
        self.with_spirc(|spirc| spirc.set_volume(level));
    }

    fn forward_events(&self, player: &Arc<Player>) {
        let mut events = player.get_player_event_channel();
        let updates = self.updates.clone();
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                let update = match event {
                    PlayerEvent::Playing { track_id, position_ms, .. } => {
                        PlayerUpdate::Playing { uri: track_id.to_uri().unwrap_or_default(), position_ms }
                    }
                    PlayerEvent::Paused { track_id, position_ms, .. } => {
                        PlayerUpdate::Paused { uri: track_id.to_uri().unwrap_or_default(), position_ms }
                    }
                    PlayerEvent::Seeked { track_id, position_ms, .. }
                    | PlayerEvent::PositionCorrection { track_id, position_ms, .. } => {
                        PlayerUpdate::Seeked { uri: track_id.to_uri().unwrap_or_default(), position_ms }
                    }
                    PlayerEvent::EndOfTrack { track_id, .. } => PlayerUpdate::EndOfTrack { uri: track_id.to_uri().unwrap_or_default() },
                    PlayerEvent::Unavailable { track_id, .. } => PlayerUpdate::Unavailable { uri: track_id.to_uri().unwrap_or_default() },
                    _ => continue,
                };
                if updates.send(update).await.is_err() {
                    break;
                }
            }
        });
    }

    fn forward_connect_events(&self, player: &Arc<Player>, session: &Session) {
        let mut events = player.get_player_event_channel();
        let updates = self.updates.clone();
        let session = session.clone();
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                let update = match event {
                    PlayerEvent::TrackChanged { audio_item } => {
                        let Some(track) = track_metadata(&session, &audio_item.uri).await else { continue };
                        ConnectUpdate::Track(track)
                    }
                    PlayerEvent::Playing { position_ms, .. } => ConnectUpdate::Playing { position_ms },
                    PlayerEvent::Paused { position_ms, .. } => ConnectUpdate::Paused { position_ms },
                    PlayerEvent::Seeked { position_ms, .. } | PlayerEvent::PositionCorrection { position_ms, .. } => {
                        ConnectUpdate::Seeked { position_ms }
                    }
                    PlayerEvent::VolumeChanged { volume } => ConnectUpdate::Volume(f64::from(volume) / f64::from(u16::MAX)),
                    PlayerEvent::SessionDisconnected { .. } | PlayerEvent::Stopped { .. } => ConnectUpdate::Inactive,
                    _ => continue,
                };
                if updates.send(PlayerUpdate::Connect(update)).await.is_err() {
                    break;
                }
            }
        });
    }
}

/// A refused save usually means the login predates the permission.
fn permission_message(error: ApiError) -> String {
    match error {
        ApiError::Forbidden(_) => "Spotify didn't allow that; log out and in again to give Tune permission".into(),
        other => other.to_string(),
    }
}

/// The track URIs of a song's radio station.
async fn station(session: &Session, seed: &str) -> Result<Vec<String>, String> {
    let body = session
        .spclient()
        .get_apollo_station("stations", seed, Some(30), Vec::new(), true)
        .await
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let mut uris = Vec::new();
    collect_track_uris(&json, &mut uris);
    Ok(uris)
}

/// Every "spotify:track:…" string in a response, in order, whatever its shape.
fn collect_track_uris(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) if s.starts_with("spotify:track:") && !out.contains(s) => out.push(s.clone()),
        serde_json::Value::Array(items) => items.iter().for_each(|item| collect_track_uris(item, out)),
        serde_json::Value::Object(map) => map.values().for_each(|item| collect_track_uris(item, out)),
        _ => {}
    }
}

/// Several songs' metadata at once, in the order given; missing ones are left out.
async fn tracks_metadata(session: &Session, uris: Vec<String>) -> Vec<Track> {
    let mut tasks = tokio::task::JoinSet::new();
    for (index, uri) in uris.into_iter().enumerate() {
        let session = session.clone();
        tasks.spawn(async move { (index, track_metadata(&session, &uri).await) });
    }
    let mut tracks: Vec<(usize, Track)> = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        if let Ok((index, Some(track))) = joined {
            tracks.push((index, track));
        }
    }
    tracks.sort_by_key(|(index, _)| *index);
    tracks.into_iter().map(|(_, track)| track).collect()
}

async fn track_metadata(session: &Session, uri: &str) -> Option<Track> {
    use librespot_metadata::Metadata;
    let id = SpotifyUri::from_uri(uri).ok()?;
    let track = librespot_metadata::Track::get(session, &id).await.ok()?;
    let cover = |wanted: &str| {
        let covers = &track.album.covers;
        covers
            .iter()
            .find(|image| format!("{:?}", image.size).eq_ignore_ascii_case(wanted))
            .or(covers.first())
            .and_then(|image| image.id.to_base16().ok())
            .map(|id| format!("https://i.scdn.co/image/{id}"))
    };
    Some(Track {
        uri: uri.to_owned(),
        name: track.name.clone(),
        artists: track.artists.iter().map(|artist| artist.name.as_str()).collect::<Vec<_>>().join(", "),
        album: track.album.name.clone(),
        duration_ms: track.duration.max(0) as u32,
        image_small: cover("SMALL"),
        image_large: cover("DEFAULT"),
        album_id: track.album.id.to_id().ok(),
        artist_refs: track
            .artists
            .iter()
            .filter_map(|artist| Some(ArtistRef { id: artist.id.to_id().ok()?, name: artist.name.clone() }))
            .collect(),
        track_number: track.number.max(0) as u32,
        added_at: None,
        position: None,
    })
}
