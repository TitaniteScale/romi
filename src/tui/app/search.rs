//! Search, the on-screen keyboard and the filter panel.

use super::input::Op;
use super::{App, Pane, Search};
use crate::filter::{self, Filter};
use fuzzy_matcher::FuzzyMatcher;
use ratatui::widgets::ListState;

/// On-screen keyboard for searching with a controller.
pub const OSK: [[&str; 13]; 3] = [
    ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M"],
    ["N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z"],
    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "␣", "⌫", "✓"],
];

/// The keyboard for a login, as a password can have anything in it. ⇧ switches the letters to capitals.
pub const LOGIN_OSK: [[&str; 13]; 6] = [
    ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m"],
    ["n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z"],
    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "␣", "⌫", "✓"],
    ["!", "@", "#", "$", "%", "^", "&", "*", "(", ")", "-", "_", "⇧"],
    ["+", "=", "[", "]", "{", "}", ";", ":", "'", "\"", ",", ".", "?"],
    ["/", "\\", "|", "~", "`", "<", ">", "", "", "", "", "", ""],
];

impl App {
    pub(super) fn filter_op(&mut self, op: Op) {
        let Some(row) = self.filter_panel else { return };
        let rows = filter::ROWS.len();
        let before = self.filter.clone();
        match op {
            Op::Up => self.filter_panel = Some((row + rows - 1) % rows),
            Op::Down => self.filter_panel = Some((row + 1) % rows),
            Op::Back | Op::Menu | Op::Filter => self.filter_panel = None,
            Op::Confirm if filter::ROWS[row] == "Clear all" => self.filter = Filter::default(),
            Op::Left => self.filter.cycle(row, false, &self.filter_options),
            Op::Right | Op::Confirm => self.filter.cycle(row, true, &self.filter_options),
            _ => {}
        }
        if self.filter != before {
            self.rebuild_keeping_selection();
            self.update_search();
        }
    }

    /// Games that show in lists: all of them, less hidden ones unless those are being shown.
    pub fn visible_count(&self) -> usize {
        (0..self.lib.games.len()).filter(|&g| self.visible(g)).count()
    }

    /// Games matching the filter, out of all of them.
    pub fn filtered_count(&self) -> usize {
        (0..self.lib.games.len())
            .filter(|&g| self.visible(g) && self.filter.matches(&self.lib.games[g], &self.state))
            .count()
    }

    /// The on-screen keyboard: d-pad moves, A types, X deletes, Y adds a space, B or ✓ closes it
    /// (the results stay, ready to browse).
    pub(super) fn osk_op(&mut self, op: Op) {
        let Some((row, col)) = self.osk else { return };
        let keys = self.osk_keys();
        let (rows, cols) = (keys.len(), keys[0].len());
        match op {
            Op::Up => self.osk = Some(((row + rows - 1) % rows, col)),
            Op::Down => self.osk = Some(((row + 1) % rows, col)),
            Op::Left => self.osk = Some((row, (col + cols - 1) % cols)),
            Op::Right => self.osk = Some((row, (col + 1) % cols)),
            Op::Random => self.edit_query(None),
            Op::Favorite => self.edit_query(Some(' ')),
            Op::Back | Op::Menu | Op::Search if self.login.is_some() => self.login_back(),
            Op::Back | Op::Menu | Op::Search if self.tag_name.is_some() => self.cancel_tag_name(),
            Op::Back | Op::Menu | Op::Search => self.close_osk(),
            Op::Confirm => match keys[row][col] {
                "" => {}
                "␣" => self.edit_query(Some(' ')),
                "⌫" => self.edit_query(None),
                "⇧" => {
                    if let Some(screen) = &mut self.login {
                        screen.shift = !screen.shift;
                    }
                }
                "✓" if self.login.is_some() => self.login_next(),
                "✓" if self.tag_name.is_some() => self.save_tag_name(),
                "✓" => self.close_osk(),
                key => {
                    let capital = self.login.as_ref().is_some_and(|l| l.shift);
                    let c = key.chars().next().map(|c| if capital { c.to_ascii_uppercase() } else { c });
                    self.edit_query(c.map(|c| if self.login.is_some() { c } else { c.to_ascii_lowercase() }));
                }
            },
            _ => {}
        }
    }

    /// The keys the on-screen keyboard has just now.
    pub fn osk_keys(&self) -> &'static [[&'static str; 13]] {
        if self.login.is_some() { &LOGIN_OSK } else { &OSK }
    }

    /// Types a character into the search (or a new tag's name), or deletes one with `None`.
    pub(super) fn edit_query(&mut self, c: Option<char>) {
        if let Some(screen) = &mut self.login {
            return screen.edit(c);
        }
        if let Some(screen) = self.cheevos.as_mut().filter(|s| s.typing) {
            match c {
                Some(c) => screen.query.push(c),
                None => {
                    screen.query.pop();
                }
            }
            screen.selected = 0;
            return;
        }
        if let Some(name) = &mut self.tag_name {
            match c {
                Some(c) => name.push(c),
                None => {
                    name.pop();
                }
            }
            return;
        }
        let Some(s) = &mut self.search else { return };
        match c {
            Some(c) => s.query.push(c),
            None => {
                s.query.pop();
            }
        }
        self.update_search();
    }

    fn close_osk(&mut self) {
        self.osk = None;
        if let Some(screen) = &mut self.cheevos {
            screen.typing = false;
            return;
        }
        // Closing with nothing typed means there's nothing to browse.
        if self.search.as_ref().is_some_and(|s| s.query.trim().is_empty()) {
            self.end_search();
        }
    }

    pub(super) fn start_search(&mut self) {
        if let Some(v) = self.views.get(self.view) {
            self.positions.insert(v.id.clone(), self.game);
        }
        self.search = Some(Search { query: String::new(), results: Vec::new() });
        self.update_search();
        self.focus = Pane::Games;
    }

    pub(super) fn end_search(&mut self) {
        if self.search.take().is_some() {
            let len = self.list().len();
            self.game = self
                .positions
                .get(&self.views.get(self.view).map(|v| v.id.clone()).unwrap_or_default())
                .copied()
                .unwrap_or(0)
                .min(len.saturating_sub(1));
            self.games_list = ListState::default();
        }
    }

    /// Fuzzy-matches across every game, best match first.
    pub(super) fn update_search(&mut self) {
        let Some(s) = &self.search else { return };
        let q = s.query.trim();
        let mut results = if q.is_empty() {
            let mut all: Vec<usize> = (0..self.lib.games.len()).collect();
            all.sort_by(|&a, &b| self.lib.games[a].sort_key.cmp(&self.lib.games[b].sort_key));
            all
        } else {
            let mut scored: Vec<(i64, usize)> = self
                .lib
                .games
                .iter()
                .enumerate()
                .filter_map(|(i, g)| {
                    let hay = format!("{} {}", g.name, g.system);
                    self.matcher.fuzzy_match(&hay, q).map(|sc| (sc, i))
                })
                .collect();
            scored.sort_by(|a, b| {
                b.0.cmp(&a.0).then_with(|| self.lib.games[a.1].sort_key.cmp(&self.lib.games[b.1].sort_key))
            });
            scored.into_iter().map(|(_, i)| i).collect()
        };
        results.retain(|&g| self.visible(g) && self.filter.matches(&self.lib.games[g], &self.state));
        if let Some(s) = &mut self.search {
            s.results = results;
        }
        self.game = 0;
        self.games_list = ListState::default();
    }
}
