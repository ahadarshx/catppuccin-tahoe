//! MPRIS: lets GNOME's media controls, media keys and shell extensions see
//! and control what Tune is playing.

use mpris_server::{LoopStatus, Metadata, PlaybackStatus, Player, Time, TrackId};

use crate::api::Track;
use crate::settings::Repeat;

pub async fn new() -> Option<Player> {
    let player = Player::builder("Tune")
        .identity("Tune")
        .desktop_entry(crate::APP_ID)
        .can_play(true)
        .can_pause(true)
        .can_go_next(true)
        .can_go_previous(true)
        .can_seek(true)
        .can_control(true)
        .can_raise(true)
        .build()
        .await
        .map_err(|e| log::warn!("MPRIS unavailable: {e}"))
        .ok()?;
    gtk::glib::spawn_future_local(player.run());
    Some(player)
}

pub async fn set_track(player: &Player, track: &Track) {
    let id = track.uri.rsplit(':').next().unwrap_or_default();
    let mut metadata = Metadata::builder()
        .title(track.name.clone())
        .artist(track.artists.split(", ").map(str::to_owned).collect::<Vec<_>>())
        .album(track.album.clone())
        .length(Time::from_millis(i64::from(track.duration_ms)));
    if let Ok(track_id) = TrackId::try_from(format!("/io/github/rice/Tune/track/{id}")) {
        metadata = metadata.trackid(track_id);
    }
    if let Some(art) = track.image_large.as_ref().or(track.image_small.as_ref()) {
        metadata = metadata.art_url(art.clone());
    }
    let _ = player.set_metadata(metadata.build()).await;
}

pub async fn set_playing(player: &Player, playing: bool) {
    let status = if playing { PlaybackStatus::Playing } else { PlaybackStatus::Paused };
    let _ = player.set_playback_status(status).await;
}

pub async fn set_repeat(player: &Player, repeat: Repeat) {
    let status = match repeat {
        Repeat::Off => LoopStatus::None,
        Repeat::All => LoopStatus::Playlist,
        Repeat::One => LoopStatus::Track,
    };
    let _ = player.set_loop_status(status).await;
}

pub fn repeat_from(status: LoopStatus) -> Repeat {
    match status {
        LoopStatus::None => Repeat::Off,
        LoopStatus::Playlist => Repeat::All,
        LoopStatus::Track => Repeat::One,
    }
}
