//! Preferences and player settings, kept in ~/.config/tune/settings.json.

use crate::backend::{AudioSettings, Quality};

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// From 0 to 1, as the slider shows it.
    pub volume: f64,
    pub muted: bool,
    pub repeat: Repeat,
    pub quality: Quality,
    pub normalize: bool,
    pub gapless: bool,
    /// Listed as a speaker in other Spotify apps.
    pub connect: bool,
    /// Closing the window while playing keeps the music going.
    pub background: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: 0.7,
            muted: false,
            repeat: Repeat::Off,
            quality: Quality::Normal,
            normalize: false,
            gapless: true,
            connect: true,
            background: false,
        }
    }
}

impl Settings {
    fn path() -> std::path::PathBuf {
        gtk::glib::user_config_dir().join("tune").join("settings.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path()).ok().and_then(|json| serde_json::from_slice(&json).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::path();
        let result = std::fs::create_dir_all(path.parent().expect("parent"))
            .map_err(|e| e.to_string())
            .and_then(|()| serde_json::to_vec_pretty(self).map_err(|e| e.to_string()))
            .and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()));
        if let Err(e) = result {
            log::warn!("could not save settings: {e}");
        }
    }

    pub fn audio(&self) -> AudioSettings {
        AudioSettings { quality: self.quality, normalize: self.normalize, gapless: self.gapless }
    }
}
