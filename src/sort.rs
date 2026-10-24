//! Orders for a system's games (and favorites), picked in the TUI and remembered per view.

use crate::library::Library;
use crate::state::State;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    Name,
    LastPlayed,
    MostPlayed,
    /// Oldest first.
    Year,
    /// The order games were favorited in; only for Favorites.
    Added,
}

const SYSTEM: &[Sort] = &[Sort::Name, Sort::LastPlayed, Sort::MostPlayed, Sort::Year];
const FAVORITES: &[Sort] = &[Sort::Added, Sort::Name, Sort::LastPlayed, Sort::MostPlayed, Sort::Year];

impl Sort {
    pub fn label(self) -> &'static str {
        match self {
            Sort::Name => "name",
            Sort::LastPlayed => "last played",
            Sort::MostPlayed => "most played",
            Sort::Year => "year",
            Sort::Added => "order added",
        }
    }

    /// The orders a view offers, its default first. Recent is always newest first, so it has none.
    pub fn options(favorites: bool) -> &'static [Sort] {
        if favorites { FAVORITES } else { SYSTEM }
    }

    /// The one after this in `options`, wrapping around.
    pub fn next(self, favorites: bool) -> Sort {
        let options = Sort::options(favorites);
        let i = options.iter().position(|&s| s == self).unwrap_or(0);
        options[(i + 1) % options.len()]
    }

    /// Whether games sorted this way fall into sections worth jumping between (letters or years).
    pub fn has_sections(self) -> bool {
        !matches!(self, Sort::LastPlayed | Sort::MostPlayed)
    }

    /// Sorts `games`, which come in name order (or order added, for favorites). Games with
    /// nothing to sort by go last, in the order they came.
    pub fn apply(self, lib: &Library, state: &State, games: &mut [usize]) {
        let key = |g: &usize| lib.games[*g].key.as_str();
        match self {
            Sort::Added => {}
            Sort::Name => games.sort_by(|&a, &b| lib.games[a].sort_key.cmp(&lib.games[b].sort_key)),
            Sort::LastPlayed => games.sort_by_key(|g| Reverse(state.last_played.get(key(g)).copied().unwrap_or(0))),
            Sort::MostPlayed => games.sort_by_key(|g| Reverse(state.playtime.get(key(g)).copied().unwrap_or(0))),
            Sort::Year => games.sort_by_key(|&g| lib.games[g].meta.year.unwrap_or(u16::MAX)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_wraps_and_stays_in_the_views_options() {
        assert_eq!(Sort::Name.next(false), Sort::LastPlayed);
        assert_eq!(Sort::Year.next(false), Sort::Name);
        assert_eq!(Sort::Year.next(true), Sort::Added);
        assert_eq!(Sort::Added.next(false), Sort::LastPlayed);
    }
}
