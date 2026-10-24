//! Things romi remembers between runs: favorites, history, play time, last selection.

use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const RECENT_LIMIT: usize = 30;

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
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
        Self::load_from(&Self::path())
    }

    fn load_from(path: &Path) -> State {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Applies a change to the state on disk and returns the result. Several romi processes can
    /// be running at once (the TUI, `romi play`, a detached game's watcher), so the change is
    /// made to a fresh read, under a lock, rather than writing out a copy that may be stale.
    pub fn update(change: impl FnOnce(&mut State)) -> State {
        Self::update_at(&Self::path(), change)
    }

    /// Best effort: losing a play-time update isn't worth interrupting anyone over, so if the
    /// lock or the write fails, the change still shows in the returned state.
    fn update_at(path: &Path, change: impl FnOnce(&mut State)) -> State {
        // Lock a separate file: state.json itself gets replaced on every write.
        let _lock = path.parent().and_then(|dir| {
            std::fs::create_dir_all(dir).ok()?;
            let file = std::fs::File::create(path.with_extension("lock")).ok()?;
            flock(&file, FlockOperation::LockExclusive).ok()?;
            Some(file)
        });
        let mut state = Self::load_from(path);
        change(&mut state);
        let tmp = path.with_extension("json.tmp");
        if let Ok(json) = serde_json::to_string_pretty(&state)
            && std::fs::write(&tmp, json).is_ok()
        {
            let _ = std::fs::rename(&tmp, path);
        }
        state
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_updates_are_not_lost() {
        let dir = std::env::temp_dir().join(format!("romi-state-test-{}", std::process::id()));
        let path = dir.join("state.json");
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        State::update_at(&path, |s| s.add_playtime("gba:a.gba", 1));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let total = State::load_from(&path).playtime["gba:a.gba"];
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(total, 200);
    }
}
