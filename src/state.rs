//! Things romi remembers between runs: favorites, history, play time, last selection.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const RECENT_LIMIT: usize = 30;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub favorites: Vec<String>,
    /// Most recent first.
    pub recent: Vec<String>,
    /// Seconds played, for games launched in wait/terminal mode.
    pub playtime: HashMap<String, u64>,
    /// Unix timestamps.
    pub last_played: HashMap<String, u64>,
    pub selected_system: Option<String>,
    pub selected_game: Option<String>,
}

impl State {
    fn path() -> PathBuf {
        crate::config::state_dir().join("state.json")
    }

    pub fn load() -> State {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Best effort: losing a play-time update isn't worth interrupting anyone over.
    pub fn save(&self) {
        let path = Self::path();
        let Some(dir) = path.parent() else { return };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let tmp = path.with_extension("json.tmp");
        if let Ok(json) = serde_json::to_string_pretty(self) {
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    pub fn is_favorite(&self, key: &str) -> bool {
        self.favorites.iter().any(|k| k == key)
    }

    pub fn toggle_favorite(&mut self, key: &str) -> bool {
        if let Some(i) = self.favorites.iter().position(|k| k == key) {
            self.favorites.remove(i);
            false
        } else {
            self.favorites.push(key.to_string());
            true
        }
    }

    pub fn record_launch(&mut self, key: &str) {
        self.recent.retain(|k| k != key);
        self.recent.insert(0, key.to_string());
        self.recent.truncate(RECENT_LIMIT);
        self.last_played.insert(key.to_string(), now());
    }

    pub fn add_playtime(&mut self, key: &str, secs: u64) {
        *self.playtime.entry(key.to_string()).or_default() += secs;
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}
