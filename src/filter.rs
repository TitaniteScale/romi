//! Narrowing the library down by genre, player count, decade, play history or your own tags.

use crate::library::{Game, Library};
use crate::state::State;
use std::collections::BTreeSet;

/// Rows of the filter panel, in order.
pub const ROWS: [&str; 6] = ["Genre", "Players", "Decade", "Played", "Tag", "Clear all"];
const PLAYERS: [Option<u8>; 4] = [None, Some(2), Some(3), Some(4)];
const PLAYED: [Option<bool>; 3] = [None, Some(false), Some(true)];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub genre: Option<String>,
    /// At least this many players.
    pub players: Option<u8>,
    /// First year of the decade, e.g. 1990.
    pub decade: Option<u16>,
    /// Some(false): never launched from romi; Some(true): launched at least once.
    pub played: Option<bool>,
    pub tag: Option<String>,
}

/// What the filter can choose from, taken from the library's metadata.
#[derive(Debug, Default)]
pub struct Options {
    /// Genres with how many games have each, most common first.
    pub genres: Vec<(String, usize)>,
    pub decades: Vec<u16>,
    /// Your tags with how many games have each, in name order.
    pub tags: Vec<(String, usize)>,
}

impl Options {
    pub fn from(lib: &Library, state: &State) -> Options {
        let decades: BTreeSet<u16> = lib.games.iter().filter_map(|g| g.meta.year).map(|y| y / 10 * 10).collect();
        let tags = state.tags.iter().map(|(t, keys)| (t.clone(), keys.len())).collect();
        Options { genres: crate::meta::genre_counts(lib), decades: decades.into_iter().collect(), tags }
    }
}

impl Filter {
    pub fn is_active(&self) -> bool {
        *self != Filter::default()
    }

    pub fn matches(&self, game: &Game, state: &State) -> bool {
        self.genre.as_ref().is_none_or(|g| game.meta.genres.contains(g))
            && self.players.is_none_or(|n| game.meta.players.is_some_and(|p| p >= n))
            && self.decade.is_none_or(|d| game.meta.year.is_some_and(|y| y / 10 * 10 == d))
            && self.played.is_none_or(|p| state.last_played.contains_key(&game.key) == p)
            && self.tag.as_ref().is_none_or(|t| state.has_tag(&game.key, t))
    }

    /// Short descriptions of what's active, e.g. ["Fighting", "2+ players"].
    pub fn labels(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.extend(self.genre.clone());
        out.extend(self.players.map(|n| format!("{n}+ players")));
        out.extend(self.decade.map(|d| format!("{d}s")));
        out.extend(self.played.map(|p| if p { "played" } else { "unplayed" }.to_string()));
        out.extend(self.tag.as_ref().map(|t| format!("#{t}")));
        out
    }

    /// The current value of a row, for display.
    pub fn value(&self, row: usize, options: &Options) -> String {
        match ROWS[row] {
            "Genre" => match &self.genre {
                Some(g) => {
                    let n = options.genres.iter().find(|(name, _)| name == g).map_or(0, |(_, n)| *n);
                    format!("{g} ({n})")
                }
                None => "Any".into(),
            },
            "Players" => self.players.map_or("Any".into(), |n| format!("{n}+")),
            "Decade" => self.decade.map_or("Any".into(), |d| format!("{d}s")),
            "Played" => match self.played {
                None => "Any".into(),
                Some(false) => "Not yet".into(),
                Some(true) => "Played".into(),
            },
            "Tag" => match &self.tag {
                Some(t) => {
                    let n = options.tags.iter().find(|(name, _)| name == t).map_or(0, |(_, n)| *n);
                    format!("{t} ({n})")
                }
                None if options.tags.is_empty() => "None yet".into(),
                None => "Any".into(),
            },
            _ => String::new(),
        }
    }

    /// Steps one row to its next (or previous) option, wrapping around through "Any".
    pub fn cycle(&mut self, row: usize, forward: bool, options: &Options) {
        match ROWS[row] {
            "Genre" => {
                let choices: Vec<Option<String>> =
                    std::iter::once(None).chain(options.genres.iter().map(|(g, _)| Some(g.clone()))).collect();
                self.genre = step(&choices, &self.genre, forward);
            }
            "Players" => self.players = step(&PLAYERS, &self.players, forward),
            "Decade" => {
                let choices: Vec<Option<u16>> =
                    std::iter::once(None).chain(options.decades.iter().copied().map(Some)).collect();
                self.decade = step(&choices, &self.decade, forward);
            }
            "Played" => self.played = step(&PLAYED, &self.played, forward),
            "Tag" => {
                let choices: Vec<Option<String>> =
                    std::iter::once(None).chain(options.tags.iter().map(|(t, _)| Some(t.clone()))).collect();
                self.tag = step(&choices, &self.tag, forward);
            }
            _ => {}
        }
    }
}

fn step<T: PartialEq + Clone>(choices: &[T], current: &T, forward: bool) -> T {
    let i = choices.iter().position(|c| c == current).unwrap_or(0);
    let n = choices.len();
    choices[if forward { (i + 1) % n } else { (i + n - 1) % n }].clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_wraps_through_any() {
        let opts = Options {
            genres: vec![("Fighting".into(), 3), ("Puzzle".into(), 1)],
            decades: vec![1990],
            tags: vec![("backlog".into(), 2)],
        };
        let mut f = Filter::default();
        f.cycle(0, true, &opts);
        assert_eq!(f.genre.as_deref(), Some("Fighting"));
        f.cycle(0, true, &opts);
        f.cycle(0, true, &opts);
        assert_eq!(f.genre, None);
        f.cycle(0, false, &opts);
        assert_eq!(f.genre.as_deref(), Some("Puzzle"));
        f.cycle(1, true, &opts);
        assert_eq!(f.labels(), ["Puzzle", "2+ players"]);
        assert_eq!(f.value(0, &opts), "Puzzle (1)");
        f.cycle(4, true, &opts);
        assert_eq!(f.value(4, &opts), "backlog (2)");
        assert_eq!(f.labels(), ["Puzzle", "2+ players", "#backlog"]);
    }
}
