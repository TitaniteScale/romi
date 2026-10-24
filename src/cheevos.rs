//! The achievements screen: your games that have achievements, narrowed by system, status and
//! name, and one game's achievements, fetched when it's opened.

use crate::config::Config;
use crate::library::Library;
use crate::ra::{Achievements, Cheevo, Info};
use std::sync::mpsc::Receiver;

/// Which games the list shows, by how far along you are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    All,
    InProgress,
    NotStarted,
    /// Beaten, completed or mastered.
    Finished,
    /// Achievements exist, but for another dump than yours.
    OtherDump,
}

const STATUSES: [Status; 5] =
    [Status::All, Status::InProgress, Status::NotStarted, Status::Finished, Status::OtherDump];

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::All => "all",
            Status::InProgress => "in progress",
            Status::NotStarted => "not started",
            Status::Finished => "finished",
            Status::OtherDump => "need another dump",
        }
    }

    pub fn step(self, forward: bool) -> Status {
        let i = STATUSES.iter().position(|&s| s == self).unwrap_or(0);
        let n = STATUSES.len();
        STATUSES[if forward { (i + 1) % n } else { (i + n - 1) % n }]
    }

    fn matches(self, a: &Info) -> bool {
        let finished = a.award.is_some() || (a.total > 0 && a.earned >= a.total);
        match self {
            Status::All => true,
            Status::InProgress => a.supported && a.earned > 0 && !finished,
            Status::NotStarted => a.supported && a.earned == 0,
            Status::Finished => a.supported && finished,
            Status::OtherDump => !a.supported,
        }
    }
}

/// One game's achievements, once they've arrived.
pub struct Open {
    pub game: usize,
    /// What was cached until the fetch comes back; None while there's nothing yet.
    pub cheevos: Option<Vec<Cheevo>>,
    pub error: Option<String>,
    /// The first achievement shown, for scrolling.
    pub top: usize,
    rx: Option<Receiver<Result<Vec<Cheevo>, String>>>,
}

#[derive(Default)]
pub struct Screen {
    /// A system's name, or None for all of them.
    pub system: Option<String>,
    pub status: Status,
    pub query: String,
    /// The name search is being typed.
    pub typing: bool,
    pub selected: usize,
    pub open: Option<Open>,
    /// Opened on one game from its menu: going back closes the screen, there being no list.
    pub just_one: bool,
}

impl Screen {
    /// The screen opened straight to one game.
    pub fn for_game(cfg: &Config, ach: &Achievements, lib: &Library, game: usize) -> Screen {
        let mut screen = Screen { just_one: true, ..Screen::default() };
        screen.open_game(cfg, ach, lib, game);
        screen
    }

    /// The games the list shows, in library order.
    pub fn rows(&self, lib: &Library, ach: &Achievements, show_hidden: bool) -> Vec<usize> {
        let words: Vec<String> = self.query.to_lowercase().split_whitespace().map(String::from).collect();
        lib.systems
            .iter()
            .filter(|s| self.system.as_ref().is_none_or(|name| &s.name == name))
            .flat_map(|s| &s.games)
            .copied()
            .filter(|&g| {
                let game = &lib.games[g];
                let name = game.name.to_lowercase();
                (show_hidden || !game.hidden)
                    && ach.games.get(&game.key).is_some_and(|a| self.status.matches(a))
                    && words.iter().all(|w| name.contains(w.as_str()))
            })
            .collect()
    }

    /// Systems with any games that have achievements, in library order.
    pub fn systems(lib: &Library, ach: &Achievements) -> Vec<String> {
        lib.systems
            .iter()
            .filter(|s| s.games.iter().any(|&g| ach.games.contains_key(&lib.games[g].key)))
            .map(|s| s.name.clone())
            .collect()
    }

    /// Steps the system filter through "all" and each system.
    pub fn step_system(&mut self, lib: &Library, ach: &Achievements, forward: bool) {
        let mut choices: Vec<Option<String>> = vec![None];
        choices.extend(Screen::systems(lib, ach).into_iter().map(Some));
        let i = choices.iter().position(|c| *c == self.system).unwrap_or(0);
        let n = choices.len();
        self.system = choices[if forward { (i + 1) % n } else { (i + n - 1) % n }].clone();
        self.selected = 0;
    }

    /// Opens a game: what's cached shows at once, and a fresh copy is fetched behind it when
    /// RetroAchievements is set up.
    pub fn open_game(&mut self, cfg: &Config, ach: &Achievements, lib: &Library, game: usize) {
        let Some(info) = ach.games.get(&lib.games[game].key) else { return };
        let id = info.id;
        let rx = crate::ra::credentials(cfg).map(|_| {
            let (tx, rx) = std::sync::mpsc::channel();
            let cfg = cfg.clone();
            std::thread::spawn(move || {
                let _ = tx.send(crate::ra::fetch_game(&cfg, id).map_err(|e| format!("{e:#}")));
            });
            rx
        });
        self.open = Some(Open { game, cheevos: crate::ra::cached_game(id), error: None, top: 0, rx });
    }

    /// Picks up a fetched game. Returns true if anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(open) = &mut self.open else { return false };
        let Some(result) = open.rx.as_ref().and_then(|rx| rx.try_recv().ok()) else { return false };
        open.rx = None;
        match result {
            Ok(cheevos) => open.cheevos = Some(cheevos),
            Err(e) => open.error = Some(e),
        }
        true
    }

    pub fn loading(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.rx.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(earned: u32, total: u32, award: Option<&str>, supported: bool) -> Info {
        Info {
            id: 1,
            title: "X".into(),
            total,
            points: 100,
            earned,
            earned_hardcore: 0,
            award: award.map(String::from),
            supported,
        }
    }

    #[test]
    fn statuses() {
        let started = info(3, 10, None, true);
        let fresh = info(0, 10, None, true);
        let done = info(10, 10, Some("mastered"), true);
        let beaten = info(4, 10, Some("beaten-softcore"), true);
        let other = info(0, 10, None, false);
        assert!(Status::InProgress.matches(&started) && !Status::InProgress.matches(&beaten));
        assert!(Status::NotStarted.matches(&fresh) && !Status::NotStarted.matches(&other));
        assert!(Status::Finished.matches(&done) && Status::Finished.matches(&beaten));
        assert!(Status::OtherDump.matches(&other) && !Status::OtherDump.matches(&fresh));
        assert!([started, fresh, done, beaten, other].iter().all(|a| Status::All.matches(a)));
        assert_eq!(Status::All.step(false), Status::OtherDump);
    }
}
