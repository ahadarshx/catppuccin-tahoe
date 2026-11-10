//! Spotify refresh tokens, kept in the user's keyring (GNOME Keyring via the
//! Secret Service). They are the only thing Tune stores to stay logged in.
//!
//! - `Web`: the login with the user's own Spotify app, for browsing.
//! - `Audio`: a login with Spotify's desktop client, which is the only one
//!   Spotify hands audio to.

#[derive(Clone, Copy)]
pub enum Kind {
    Web,
    Audio,
}

impl Kind {
    fn attributes(self) -> [(&'static str, &'static str); 2] {
        // "spotify-refresh-token" is what the first version stored, a desktop-client login.
        let kind = match self {
            // Bumped when Tune asks for new permissions, so the next start logs in again.
            Self::Web => "spotify-web-refresh-token-v3",
            Self::Audio => "spotify-refresh-token",
        };
        [("application", "tune"), ("kind", kind)]
    }
}

pub async fn load(kind: Kind) -> Option<String> {
    let keyring = oo7::Keyring::new().await.ok()?;
    let _ = keyring.unlock().await;
    let items = keyring.search_items(&kind.attributes()).await.ok()?;
    let secret = items.first()?.secret().await.ok()?;
    String::from_utf8(secret.as_bytes().to_vec()).ok().filter(|token| !token.is_empty())
}

pub async fn save(kind: Kind, refresh_token: &str) -> Result<(), String> {
    let keyring = oo7::Keyring::new().await.map_err(|e| e.to_string())?;
    keyring
        .create_item("Tune Spotify login", &kind.attributes(), refresh_token, true)
        .await
        .map_err(|e| e.to_string())
}

pub async fn clear(kind: Kind) {
    if let Ok(keyring) = oo7::Keyring::new().await {
        let _ = keyring.delete(&kind.attributes()).await;
    }
}
