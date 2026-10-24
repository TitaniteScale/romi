//! Play statistics: totals per system, the most-played games, and a year month by month.
//!
//! Totals come from state.json's play time, which goes back as far as romi does. Months need
//! to know when each session was, so every session is also appended to `history.jsonl` in the
//! state folder; the year view covers what's been recorded there.

use crate::config::state_dir;
use crate::library::Library;
use crate::state::State;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

/// One play session, as a line of `history.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Played {
    pub key: String,
    /// Unix time it started.
    pub at: u64,
    pub secs: u64,
}

fn history_path() -> PathBuf {
    state_dir().join("history.jsonl")
}

/// Appends a session to the history. Best effort, like the rest of the state; one short line
/// per write, so several romi processes can append at once.
pub fn record(key: &str, at: u64, secs: u64) {
    let Ok(line) = serde_json::to_string(&Played { key: key.to_string(), at, secs }) else { return };
    let _ = std::fs::create_dir_all(state_dir());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(history_path()) {
        let _ = f.write_all(format!("{line}\n").as_bytes());
    }
}

/// Every recorded session, oldest first. Lines that don't parse are skipped.
pub fn history() -> Vec<Played> {
    let text = std::fs::read_to_string(history_path()).unwrap_or_default();
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

#[derive(Debug, Default, Serialize)]
pub struct Stats {
    pub total_secs: u64,
    /// Games with any play time.
    pub games: usize,
    /// Play time per system, most first, by system name.
    pub systems: Vec<(String, u64)>,
    /// Most-played games, most first, by name.
    pub top: Vec<(String, u64)>,
    pub year: Year,
}

/// One calendar year (local time) from the history.
#[derive(Debug, Default, Serialize)]
pub struct Year {
    pub year: i32,
    /// Seconds played in each month, January first.
    pub months: [u64; 12],
    pub sessions: usize,
    /// Its most-played games, most first.
    pub top: Vec<(String, u64)>,
    /// When the history starts (Unix time), if there's any.
    pub since: Option<u64>,
}

const TOP: usize = 10;

impl Stats {
    pub fn compute(lib: &Library, state: &State, history: &[Played], now: u64) -> Stats {
        let (name, system) = names(lib);
        let mut systems: HashMap<String, u64> = HashMap::new();
        let mut games: Vec<(String, u64)> = Vec::new();
        for (key, &secs) in state.playtime.iter().filter(|(_, s)| **s > 0) {
            *systems.entry(system(key)).or_default() += secs;
            games.push((name(key), secs));
        }

        let year = local_year_month(now).0;
        let mut y = Year { year, since: history.first().map(|p| p.at), ..Year::default() };
        let mut year_games: HashMap<String, u64> = HashMap::new();
        for p in history {
            let (py, month) = local_year_month(p.at);
            if py == year {
                y.months[month as usize - 1] += p.secs;
                y.sessions += 1;
                *year_games.entry(name(&p.key)).or_default() += p.secs;
            }
        }
        y.top = most(year_games.into_iter().collect());

        Stats {
            total_secs: games.iter().map(|(_, s)| s).sum(),
            games: games.len(),
            systems: most(systems.into_iter().collect()),
            top: most(games),
            year: y,
        }
    }
}

/// Sorted most first (then by name), cut to the top few.
fn most(mut v: Vec<(String, u64)>) -> Vec<(String, u64)> {
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(TOP);
    v
}

/// Lookups from a game key to its name and its system's name. Games no longer in the library
/// still count: they go by their key, under the system their key names (by id if it's gone too).
fn names(lib: &Library) -> (impl Fn(&str) -> String + '_, impl Fn(&str) -> String + '_) {
    let by_key: HashMap<&str, usize> = lib.games.iter().enumerate().map(|(i, g)| (g.key.as_str(), i)).collect();
    let alias: HashMap<&str, usize> =
        lib.games.iter().enumerate().flat_map(|(i, g)| g.disc_keys.iter().map(move |k| (k.as_str(), i))).collect();
    let find = move |key: &str| by_key.get(key).or_else(|| alias.get(key)).copied();
    let find2 = find.clone();
    let name = move |key: &str| match find(key) {
        Some(g) => lib.games[g].name.clone(),
        None => key.split_once(':').map_or(key, |(_, rest)| rest).to_string(),
    };
    let system = move |key: &str| match find2(key) {
        Some(g) => lib.games[g].system.clone(),
        None => {
            let id = key.split_once(':').map_or("other", |(id, _)| id);
            lib.systems.iter().find(|s| s.id == id).map_or_else(|| id.to_string(), |s| s.name.clone())
        }
    };
    (name, system)
}

/// The local calendar year and month (1-12) of a Unix time.
pub fn local_year_month(t: u64) -> (i32, u32) {
    let t = t as libc::time_t;
    // SAFETY: localtime_r only writes the tm it's given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return (1970, 1);
    }
    (tm.tm_year + 1900, tm.tm_mon as u32 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_and_years() {
        let mut state = State::default();
        state.playtime.insert("snes:a.sfc".into(), 3600);
        state.playtime.insert("snes:b.sfc".into(), 60);
        state.playtime.insert("gba:c.gba".into(), 600);
        state.playtime.insert("gba:never.gba".into(), 0);
        // Mid-month, so time zones can't move them across a month or year.
        let mar_2026 = 1_773_576_000; // 2026-03-15
        let jun_2026 = 1_781_524_800; // 2026-06-15
        let dec_2025 = 1_765_800_000; // 2025-12-15
        let history = vec![
            Played { key: "gba:c.gba".into(), at: dec_2025, secs: 100 },
            Played { key: "snes:a.sfc".into(), at: mar_2026, secs: 3000 },
            Played { key: "gba:c.gba".into(), at: jun_2026, secs: 500 },
        ];
        let s = Stats::compute(&Library::default(), &state, &history, jun_2026);
        assert_eq!(s.total_secs, 4260);
        assert_eq!(s.games, 3);
        assert_eq!(s.systems, [("snes".to_string(), 3660), ("gba".to_string(), 600)]);
        assert_eq!(s.top[0], ("a.sfc".to_string(), 3600));
        assert_eq!(s.year.year, 2026);
        assert_eq!(s.year.months[2], 3000);
        assert_eq!(s.year.months[5], 500);
        assert_eq!(s.year.sessions, 2);
        assert_eq!(s.year.top, [("a.sfc".to_string(), 3000), ("c.gba".to_string(), 500)]);
        assert_eq!(s.year.since, Some(dec_2025));
    }
}
