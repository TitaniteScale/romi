//! Things romi remembers between runs: favorites, history, play time, last selection.

use crate::sort::Sort;
use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
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
    /// How each view is sorted, by view id, when it isn't the default.
    pub sort: HashMap<String, Sort>,
    /// Your own tags, each with its games in the order they were tagged.
    pub tags: BTreeMap<String, Vec<String>>,
    /// Games shown as a grid of covers rather than a list.
    pub grid: bool,
    /// The controllers in player order, player 1's first, from the players screen, by name (see
    /// `emu::controls::pad_names`).
    pub players: Vec<String>,
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
        if let Ok(json) = serde_json::to_string_pretty(&state) {
            let _ = crate::files::write_atomic(path, json);
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

    pub fn has_tag(&self, key: &str, tag: &str) -> bool {
        self.tags.get(tag).is_some_and(|keys| keys.iter().any(|k| k == key))
    }

    /// A game's tags, in name order.
    pub fn tags_of(&self, key: &str) -> Vec<&str> {
        self.tags.iter().filter(|(_, keys)| keys.iter().any(|k| k == key)).map(|(t, _)| t.as_str()).collect()
    }

    /// The tag `name` means: an existing one in any case, or `name` trimmed as a new one.
    pub fn tag_named(&self, name: &str) -> String {
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        self.tags.keys().find(|t| t.eq_ignore_ascii_case(&name)).cloned().unwrap_or(name)
    }

    /// Tags or untags a game, returning whether it's tagged now. A tag with no games left goes.
    pub fn toggle_tag(&mut self, key: &str, tag: &str) -> bool {
        let keys = self.tags.entry(tag.to_string()).or_default();
        let added = match keys.iter().position(|k| k == key) {
            Some(i) => {
                keys.remove(i);
                false
            }
            None => {
                keys.push(key.to_string());
                true
            }
        };
        if keys.is_empty() {
            self.tags.remove(tag);
        }
        added
    }

    /// Whether anything is remembered about `key`.
    pub fn mentions(&self, key: &str) -> bool {
        self.tags.values().any(|keys| keys.iter().any(|k| k == key))
            || self.is_favorite(key)
            || self.recent.iter().any(|k| k == key)
            || self.playtime.contains_key(key)
            || self.last_played.contains_key(key)
            || self.selected_game.as_deref() == Some(key)
    }

    /// Moves everything remembered about `from` to `to`, merging with what `to` already has.
    pub fn rename_key(&mut self, from: &str, to: &str) {
        if let Some(i) = self.favorites.iter().position(|k| k == from) {
            self.favorites.remove(i);
            if !self.is_favorite(to) {
                self.favorites.push(to.to_string());
            }
        }
        // Keep `to` at whichever of the two places was more recent.
        let mut seen = false;
        self.recent.retain_mut(|k| {
            if k == from {
                *k = to.to_string();
            }
            let first = !(k == to && seen);
            seen |= k == to;
            first
        });
        if let Some(secs) = self.playtime.remove(from) {
            self.add_playtime(to, secs);
        }
        if let Some(at) = self.last_played.remove(from) {
            let latest = self.last_played.entry(to.to_string()).or_default();
            *latest = (*latest).max(at);
        }
        if self.selected_game.as_deref() == Some(from) {
            self.selected_game = Some(to.to_string());
        }
        for keys in self.tags.values_mut() {
            if let Some(i) = keys.iter().position(|k| k == from) {
                keys.remove(i);
                if !keys.iter().any(|k| k == to) {
                    keys.insert(i, to.to_string());
                }
            }
        }
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renaming_merges_into_the_new_key() {
        let mut s = State {
            favorites: vec!["psx:a (Disc 2).chd".into()],
            recent: vec!["snes:x.sfc".into(), "psx:a (Disc 2).chd".into(), "psx:a.m3u".into()],
            playtime: [("psx:a (Disc 2).chd".into(), 60), ("psx:a.m3u".into(), 30)].into(),
            last_played: [("psx:a (Disc 2).chd".into(), 200), ("psx:a.m3u".into(), 100)].into(),
            selected_system: None,
            selected_game: Some("psx:a (Disc 2).chd".into()),
            sort: HashMap::new(),
            tags: [("backlog".into(), vec!["psx:a (Disc 2).chd".into()])].into(),
            grid: false,
            players: Vec::new(),
        };
        assert!(s.mentions("psx:a (Disc 2).chd"));
        s.rename_key("psx:a (Disc 2).chd", "psx:a.m3u");
        assert!(!s.mentions("psx:a (Disc 2).chd"));
        assert_eq!(s.favorites, ["psx:a.m3u"]);
        assert_eq!(s.recent, ["snes:x.sfc", "psx:a.m3u"]);
        assert_eq!(s.playtime["psx:a.m3u"], 90);
        assert_eq!(s.last_played["psx:a.m3u"], 200);
        assert_eq!(s.selected_game.as_deref(), Some("psx:a.m3u"));
        assert_eq!(s.tags["backlog"], ["psx:a.m3u"]);
    }

    #[test]
    fn tags_come_and_go() {
        let mut s = State::default();
        let tag = s.tag_named("  couch   co-op ");
        assert_eq!(tag, "couch co-op");
        assert!(s.toggle_tag("snes:a.sfc", &tag));
        assert_eq!(s.tag_named("Couch Co-op"), "couch co-op");
        assert!(s.toggle_tag("gba:b.gba", "couch co-op"));
        assert_eq!(s.tags_of("gba:b.gba"), ["couch co-op"]);
        assert!(!s.toggle_tag("snes:a.sfc", "couch co-op"));
        assert!(!s.toggle_tag("gba:b.gba", "couch co-op"));
        assert!(s.tags.is_empty());
    }

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
