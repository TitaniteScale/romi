//! A game being played: what romi records when it starts and when it stops, and the list of
//! games running right now that `romi status` reads. The TUI and `romi play` both go through
//! here, so history and play time are kept the same way.

use crate::library::Library;
use crate::state::{State, now};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A running game, as kept in `<state dir>/running/<pid>.json` while it plays.
#[derive(Debug, Serialize, Deserialize)]
pub struct Running {
    pub key: String,
    pub name: String,
    /// System id.
    pub system: String,
    pub system_name: String,
    /// The game's process.
    pub pid: u32,
    /// Unix time.
    pub started: u64,
    /// The process's start time in clock ticks since boot, so a file left behind by a crash isn't
    /// mistaken for a live game once its pid is reused.
    proc_start: Option<u64>,
}

pub struct Session {
    key: String,
    started: Instant,
    file: PathBuf,
}

impl Session {
    /// Call once game `g` has started as process `pid`. Returns the updated state.
    pub fn begin(lib: &Library, g: usize, pid: u32) -> (Session, State) {
        let game = &lib.games[g];
        let system = lib.system_of(g);
        let record = Running {
            key: game.key.clone(),
            name: game.name.clone(),
            system: system.id.clone(),
            system_name: system.name.clone(),
            pid,
            started: now(),
            proc_start: proc_stat(pid).map(|(_, start)| start),
        };
        let file = running_dir().join(format!("{pid}.json"));
        // Best effort, like the rest of the state: a game that can't be listed still plays.
        let _ = write_atomic(&file, &record);
        let state = State::update(|s| s.record_launch(&game.key));
        (Session { key: game.key.clone(), started: Instant::now(), file }, state)
    }

    /// Call once the game has exited. Returns how long it ran and the updated state.
    pub fn end(self) -> (Duration, State) {
        let _ = std::fs::remove_file(&self.file);
        let played = self.started.elapsed();
        let state = State::update(|s| s.add_playtime(&self.key, played.as_secs()));
        (played, state)
    }
}

fn running_dir() -> PathBuf {
    crate::config::state_dir().join("running")
}

fn write_atomic(path: &Path, record: &Running) -> Result<()> {
    std::fs::create_dir_all(path.parent().context("no parent")?)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(record)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The games running right now, oldest first. Files for games that are no longer running (romi
/// was killed before it could clean up) are removed along the way.
pub fn running() -> Vec<Running> {
    let Ok(entries) = std::fs::read_dir(running_dir()) else { return Vec::new() };
    let mut out: Vec<Running> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|path| {
            let record = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Running>(&b).ok());
            match record {
                Some(r) if is_alive(&r) => Some(r),
                _ => {
                    let _ = std::fs::remove_file(&path);
                    None
                }
            }
        })
        .collect();
    out.sort_by_key(|r| r.started);
    out
}

fn is_alive(r: &Running) -> bool {
    match proc_stat(r.pid) {
        Some((state, start)) => state != 'Z' && r.proc_start.is_none_or(|s| s == start),
        None => false,
    }
}

/// A process's state letter and start time, from `/proc/<pid>/stat`.
fn proc_stat(pid: u32) -> Option<(char, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name in parentheses may contain spaces, so count fields after the last ')'.
    let mut fields = stat[stat.rfind(')')? + 1..].split_whitespace();
    let state = fields.next()?.chars().next()?;
    // starttime is field 22 overall; state is field 3.
    let start = fields.nth(18)?.parse().ok()?;
    Some((state, start))
}

/// Starts `romi play <key> --mode wait` in the background, which plays the game and records the
/// session however long it runs, whether or not whoever started it is still around. This is how
/// detached games get tracked. It has its own process group, so closing the terminal doesn't
/// stop it.
pub fn play_in_background(cfg_path: &Path, key: &str) -> Result<Child> {
    let exe = std::env::current_exe().context("finding romi's own path")?;
    Command::new(&exe)
        .arg("--config")
        .arg(cfg_path)
        .args(["play", "--mode", "wait", "--", key])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_own_process_stat() {
        let (state, start) = proc_stat(std::process::id()).unwrap();
        assert_eq!(state, 'R');
        assert!(start > 0);
        assert_eq!(proc_stat(u32::MAX), None);
    }
}
