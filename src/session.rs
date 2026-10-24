//! A game being played: what romi records when it starts and when it stops. The TUI and
//! `romi play` both go through here, so history and play time are kept the same way.

use crate::state::State;
use std::time::{Duration, Instant};

pub struct Session {
    key: String,
    started: Instant,
}

impl Session {
    /// Call once the game's process has started. Returns the updated state.
    pub fn begin(key: &str) -> (Session, State) {
        let state = State::update(|s| s.record_launch(key));
        (Session { key: key.to_string(), started: Instant::now() }, state)
    }

    /// Call once the game has exited. Returns how long it ran and the updated state.
    pub fn end(self) -> (Duration, State) {
        let played = self.started.elapsed();
        let state = State::update(|s| s.add_playtime(&self.key, played.as_secs()));
        (played, state)
    }
}
