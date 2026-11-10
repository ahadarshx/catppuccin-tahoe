//! Lyrics from LRCLIB (lrclib.net), a free and open lyrics database:
//! time-synced when it has them, plain otherwise. Kept on disk, so a song's
//! lyrics show at once the next time.

use serde_json::Value;

use crate::api::Track;

const BASE: &str = "https://lrclib.net/api";
/// LRCLIB asks clients to say who they are.
const USER_AGENT: &str = concat!("Tune/", env!("CARGO_PKG_VERSION"), " (GNOME Spotify player)");
/// How far a match's length may be from the song's, in seconds.
const DURATION_SLACK: f64 = 3.0;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Lyrics {
    /// Lines with their start time, in order; empty when only plain lyrics exist.
    pub synced: Vec<(u32, String)>,
    pub plain: String,
    pub instrumental: bool,
}

impl Lyrics {
    fn from_record(record: &Value) -> Option<Self> {
        let synced = record["syncedLyrics"].as_str().map(parse_lrc).unwrap_or_default();
        let plain = record["plainLyrics"].as_str().unwrap_or_default().trim().to_owned();
        let instrumental = record["instrumental"].as_bool().unwrap_or(false);
        (instrumental || !synced.is_empty() || !plain.is_empty()).then_some(Self { synced, plain, instrumental })
    }
}

/// "[01:23.45]words" lines, a line may carry several times; metadata tags
/// like "[ar:…]" are skipped.
pub fn parse_lrc(text: &str) -> Vec<(u32, String)> {
    let mut lines = Vec::new();
    for line in text.lines() {
        let mut rest = line.trim();
        let mut times = Vec::new();
        while let Some(tag) = rest.strip_prefix('[') {
            let Some(end) = tag.find(']') else { break };
            if let Some(ms) = parse_time(&tag[..end]) {
                times.push(ms);
            }
            rest = &tag[end + 1..];
        }
        let words = rest.trim().to_owned();
        lines.extend(times.into_iter().map(|ms| (ms, words.clone())));
    }
    lines.sort_by_key(|(ms, _)| *ms);
    lines
}

fn parse_time(tag: &str) -> Option<u32> {
    let (minutes, seconds) = tag.split_once(':')?;
    let minutes: u32 = minutes.trim().parse().ok()?;
    let seconds: f64 = seconds.trim().parse().ok()?;
    Some(minutes * 60_000 + (seconds * 1000.0).round() as u32)
}

fn cache_path(track: &Track) -> Option<std::path::PathBuf> {
    let id = track.uri.rsplit(':').next().filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric()))?;
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".cache")))?;
    Some(cache.join("tune/lyrics").join(format!("{id}.json")))
}

/// The song's lyrics, or None when LRCLIB has none.
pub async fn fetch(http: &reqwest::Client, track: &Track) -> Result<Option<Lyrics>, String> {
    let path = cache_path(track);
    if let Some(path) = &path
        && let Ok(json) = tokio::fs::read(path).await
        && let Ok(lyrics) = serde_json::from_slice::<Lyrics>(&json)
    {
        return Ok(Some(lyrics));
    }
    // The exact lookup is often busy (it asks other sources on a miss);
    // the search still answers then.
    let exact = exact(http, track).await.unwrap_or_else(|e| {
        log::info!("{e}; searching instead");
        None
    });
    let lyrics = match exact {
        Some(lyrics) => Some(lyrics),
        None => search(http, track).await?,
    };
    if let (Some(path), Some(lyrics)) = (path, &lyrics)
        && let Ok(json) = serde_json::to_vec(lyrics)
    {
        let _ = tokio::fs::create_dir_all(path.parent().expect("parent")).await;
        let _ = tokio::fs::write(path, json).await;
    }
    Ok(lyrics)
}

fn first_artist(track: &Track) -> String {
    track.artist_refs.first().map(|a| a.name.clone()).unwrap_or_else(|| track.artists.split(", ").next().unwrap_or_default().to_owned())
}

async fn request(http: &reqwest::Client, path: &str, params: &[(&str, String)]) -> Result<Option<Value>, String> {
    let url = reqwest::Url::parse_with_params(&format!("{BASE}{path}"), params).map_err(|e| e.to_string())?;
    let response = http
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(std::time::Duration::from_secs(12))
        .send()
        .await
        .map_err(|e| if e.is_timeout() { "LRCLIB took too long to answer".to_owned() } else { "Couldn't reach LRCLIB; check your connection".to_owned() })?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("LRCLIB is having trouble right now ({})", response.status().as_u16()));
    }
    response.json().await.map(Some).map_err(|_| "LRCLIB sent something Tune couldn't read".to_owned())
}

/// LRCLIB's exact lookup: title, artist, album and length must all agree.
async fn exact(http: &reqwest::Client, track: &Track) -> Result<Option<Lyrics>, String> {
    let params = [
        ("track_name", track.name.clone()),
        ("artist_name", first_artist(track)),
        ("album_name", track.album.clone()),
        ("duration", (track.duration_ms / 1000).to_string()),
    ];
    Ok(request(http, "/get", &params).await?.as_ref().and_then(Lyrics::from_record))
}

/// A looser search, for albums named differently; the closest length
/// wins, and synced lyrics beat plain ones.
async fn search(http: &reqwest::Client, track: &Track) -> Result<Option<Lyrics>, String> {
    // "Song - Remastered 2011" and "Song (feat. X)" are listed as "Song".
    let name = track.name.split(" - ").next().unwrap_or(&track.name);
    let name = name.split(" (feat").next().unwrap_or(name).trim().to_owned();
    let params = [("track_name", name), ("artist_name", first_artist(track))];
    let Some(Value::Array(records)) = request(http, "/search", &params).await? else { return Ok(None) };
    let length = f64::from(track.duration_ms) / 1000.0;
    let best = records
        .iter()
        .filter(|r| (r["duration"].as_f64().unwrap_or(0.0) - length).abs() <= DURATION_SLACK)
        .filter_map(|r| Some((r, Lyrics::from_record(r)?)))
        .max_by_key(|(r, lyrics)| {
            let off = ((r["duration"].as_f64().unwrap_or(0.0) - length).abs() * 10.0) as i64;
            (!lyrics.synced.is_empty(), -off)
        });
    Ok(best.map(|(_, lyrics)| lyrics))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc() {
        let lines = parse_lrc("[ar:Someone]\n[00:01.50]First\n[00:10.00][01:00.25]Chorus\n[00:05.00]\n");
        assert_eq!(
            lines,
            vec![(1500, "First".into()), (5000, String::new()), (10_000, "Chorus".into()), (60_250, "Chorus".into())]
        );
    }
}
