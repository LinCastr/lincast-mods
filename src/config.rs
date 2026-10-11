//! Preferências do app em ~/.config/linex-mods/config.json.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    /// "default" ou "wallust".
    #[serde(default)]
    pub theme: String,
    /// AppID do último jogo aberto.
    #[serde(default)]
    pub last_game: Option<u32>,
    /// Interface language: "auto" (empty), "en" or "pt".
    #[serde(default)]
    pub lang: String,
}

fn path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::steam::home().join(".config"))
        .join("linex-mods/config.json")
}

pub fn load() -> Config {
    fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(c: &Config) {
    let p = path();
    let _ = fs::create_dir_all(p.parent().unwrap());
    if let Ok(s) = serde_json::to_string_pretty(c) {
        let _ = fs::write(p, s);
    }
}
