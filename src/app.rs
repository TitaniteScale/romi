//! App state and input handling. Keyboard and controller both funnel into the same `Op`s.

use crate::config::{self, Config, LaunchMode};
use crate::cover::Covers;
use crate::filter::{self, Filter};
use crate::gamepad::{PadAction, PadEvent};
use crate::launch::Prepared;
use crate::library::Library;
use crate::state::State;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

const STATUS_TTL: Duration = Duration::from_secs(6);
/// How long the big letter stays up after a letter jump.
const FLASH_TTL: Duration = Duration::from_millis(700);

/// The controller menu (Start). Everything that would otherwise need a keyboard lives here.
pub const MENU: &[&str] = &["Search", "Filter", "Random game", "Details pane", "Reload config", "Quit romi"];

/// On-screen keyboard for searching with a controller.
pub const OSK: [[&str; 13]; 3] = [
    ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M"],
    ["N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z"],
    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "␣", "⌫", "✓"],
];
/// After a game exits, ignore input briefly so the button used to quit doesn't launch something.
const INPUT_COOLDOWN: Duration = Duration::from_millis(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Systems,
    Games,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewKind {
    Favorites,
    Recent,
    System(usize),
}

/// One entry in the systems pane: a real system or a pseudo one like Favorites.
#[derive(Debug, Clone)]
pub struct View {
    pub kind: ViewKind,
    pub id: String,
    pub name: String,
    pub games: Vec<usize>,
}

pub struct Search {
    pub query: String,
    pub results: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

pub struct Status {
    pub text: String,
    pub level: Level,
    at: Instant,
}

pub struct Running {
    pub game: usize,
    pub child: Child,
    pub started: Instant,
}

pub enum PadState {
    Disabled,
    Searching,
    Connected(String),
    Unavailable(String),
}

/// A command that needs the real terminal; the main loop runs it since it owns the terminal.
pub struct TerminalJob {
    pub game: usize,
    pub prepared: Prepared,
}

enum Op {
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Left,
    Right,
    Confirm,
    Back,
    NextSystem,
    PrevSystem,
    NextLetter,
    PrevLetter,
    Favorite,
    Random,
    ToggleDetails,
    Menu,
    Search,
    Filter,
}

pub struct App {
    pub cfg: Config,
    pub cfg_path: PathBuf,
    pub lib: Library,
    pub state: State,
    pub views: Vec<View>,
    pub view: usize,
    pub game: usize,
    pub focus: Pane,
    pub search: Option<Search>,
    pub status: Option<Status>,
    pub running: Option<Running>,
    pub pad: PadState,
    pub show_details: bool,
    pub terminal_job: Option<TerminalJob>,
    /// Box art, when enabled and the terminal could be queried.
    pub covers: Option<Covers>,
    /// Selected item while the controller menu is open.
    pub menu: Option<usize>,
    /// Cursor (row, column) while the on-screen keyboard is open.
    pub osk: Option<(usize, usize)>,
    /// Letter shown briefly after a letter jump.
    flash: Option<(char, Instant)>,
    pub filter: Filter,
    pub filter_options: filter::Options,
    /// Selected row while the filter panel is open.
    pub filter_panel: Option<usize>,
    /// Database metadata being looked up in the background.
    metadata: Option<Receiver<Vec<crate::meta::Found>>>,
    pub quit: bool,
    pub systems_list: ListState,
    pub games_list: ListState,
    /// Visible rows in the games list, set while drawing; used for paging.
    pub page: usize,
    term_focused: bool,
    cooldown_until: Option<Instant>,
    /// Remembered game selection per view, so hopping between systems doesn't lose your place.
    positions: HashMap<String, usize>,
    matcher: SkimMatcherV2,
}

pub fn log_path() -> PathBuf {
    config::state_dir().join("last.log")
}

impl App {
    pub fn new(cfg: Config, cfg_path: PathBuf) -> App {
        let lib = Library::load(&cfg);
        let state = State::load();
        let pad = if cfg.controller.enabled { PadState::Searching } else { PadState::Disabled };
        let mut app = App {
            show_details: cfg.theme.show_details,
            cfg,
            cfg_path,
            lib,
            state,
            views: Vec::new(),
            view: 0,
            game: 0,
            focus: Pane::Games,
            search: None,
            status: None,
            running: None,
            pad,
            terminal_job: None,
            covers: None,
            menu: None,
            osk: None,
            flash: None,
            metadata: None,
            filter: Filter::default(),
            filter_options: filter::Options::default(),
            filter_panel: None,
            quit: false,
            systems_list: ListState::default(),
            games_list: ListState::default(),
            page: 10,
            term_focused: true,
            cooldown_until: None,
            positions: HashMap::new(),
            matcher: SkimMatcherV2::default(),
        };
        app.load_metadata();
        app.filter_options = filter::Options::from(&app.lib);
        app.rebuild_views();
        let (sys, game) = (app.state.selected_system.clone(), app.state.selected_game.clone());
        app.select(sys.as_deref(), game.as_deref());
        for w in std::mem::take(&mut app.lib.warnings) {
            app.set_status(Level::Warn, w);
        }
        app
    }

    /// Starts looking up database metadata off the UI thread; `tick` applies it when it's ready.
    /// Replacing the receiver (on reload) makes any lookup still running for the old library
    /// send into the void.
    fn load_metadata(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (cfg, lib) = (self.cfg.clone(), self.lib.clone());
        std::thread::Builder::new()
            .name("metadata".into())
            .spawn(move || {
                let _ = tx.send(crate::meta::lookup_all(&cfg, &lib));
            })
            .ok();
        self.metadata = Some(rx);
    }

    // ---- views & selection ----

    fn rebuild_views(&mut self) {
        let keep = |g: &usize| self.filter.matches(&self.lib.games[*g], &self.state);
        let resolve = |keys: &[String]| keys.iter().filter_map(|k| self.lib.find(k)).filter(keep).collect::<Vec<_>>();
        let mut views = Vec::new();
        let favs = resolve(&self.state.favorites);
        if !favs.is_empty() {
            views.push(View {
                kind: ViewKind::Favorites,
                id: "@favorites".into(),
                name: "Favorites".into(),
                games: favs,
            });
        }
        let recent = resolve(&self.state.recent);
        if !recent.is_empty() {
            views.push(View { kind: ViewKind::Recent, id: "@recent".into(), name: "Recent".into(), games: recent });
        }
        for (i, s) in self.lib.systems.iter().enumerate() {
            let games: Vec<usize> = s.games.iter().copied().filter(keep).collect();
            // While filtering, systems with nothing left would just be noise.
            if !games.is_empty() || !self.filter.is_active() {
                views.push(View { kind: ViewKind::System(i), id: s.id.clone(), name: s.name.clone(), games });
            }
        }
        self.views = views;
    }

    /// Rebuilds views while keeping the cursor on the same system and game.
    fn rebuild_keeping_selection(&mut self) {
        let sys = self.views.get(self.view).map(|v| v.id.clone());
        let game = self.selected_game().map(|g| self.lib.games[g].key.clone());
        self.rebuild_views();
        self.select(sys.as_deref(), game.as_deref());
    }

    fn select(&mut self, view_id: Option<&str>, game_key: Option<&str>) {
        self.view = view_id.and_then(|id| self.views.iter().position(|v| v.id == id)).unwrap_or(0);
        let list = self.views.get(self.view).map(|v| v.games.as_slice()).unwrap_or(&[]);
        self.game = game_key
            .and_then(|k| list.iter().position(|&g| self.lib.games[g].key == k))
            .unwrap_or(0)
            .min(list.len().saturating_sub(1));
    }

    /// Games currently listed in the right pane: search results or the selected view.
    pub fn list(&self) -> &[usize] {
        match &self.search {
            Some(s) => &s.results,
            None => self.views.get(self.view).map(|v| v.games.as_slice()).unwrap_or(&[]),
        }
    }

    pub fn selected_game(&self) -> Option<usize> {
        self.list().get(self.game).copied()
    }

    pub fn current_view(&self) -> Option<&View> {
        self.views.get(self.view)
    }

    /// The real system a game belongs to, for problem reporting.
    pub fn system_problem(&self, game: usize) -> Option<&str> {
        self.lib.systems.iter().find(|s| s.games.contains(&game)).and_then(|s| s.problem.as_deref())
    }

    fn switch_view(&mut self, to: usize) {
        if self.views.is_empty() {
            return;
        }
        if let Some(v) = self.views.get(self.view) {
            self.positions.insert(v.id.clone(), self.game);
        }
        self.view = to % self.views.len();
        let len = self.views[self.view].games.len();
        self.game = self.positions.get(&self.views[self.view].id).copied().unwrap_or(0).min(len.saturating_sub(1));
        self.games_list = ListState::default();
    }

    fn move_cursor(&mut self, delta: isize) {
        match self.focus {
            Pane::Systems if self.search.is_none() => {
                let n = self.views.len() as isize;
                if n > 0 {
                    let to = (self.view as isize + delta).clamp(0, n - 1) as usize;
                    if to != self.view {
                        self.switch_view(to);
                    }
                }
            }
            _ => {
                let n = self.list().len() as isize;
                if n > 0 {
                    self.game = (self.game as isize + delta).clamp(0, n - 1) as usize;
                }
            }
        }
    }

    /// The letter a game is filed under: first letter ignoring "The", or `#` for anything else.
    fn letter(&self, g: usize) -> char {
        match self.lib.games[g].sort_key.chars().find(|c| c.is_alphanumeric()) {
            Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
            _ => '#',
        }
    }

    /// Moves to the first game of the next letter, or back to the start of this one (then the
    /// previous letter), like a phone's contact list.
    fn jump_letter(&mut self, forward: bool) {
        let list = self.list().to_vec();
        if list.is_empty() {
            return;
        }
        self.focus = Pane::Games;
        let here = self.letter(list[self.game]);
        let group_start = |end: usize, l: char| (0..=end).rev().take_while(|&i| self.letter(list[i]) == l).last();
        let target = if forward {
            (self.game + 1..list.len()).find(|&i| self.letter(list[i]) != here)
        } else {
            match group_start(self.game, here) {
                Some(start) if start < self.game => Some(start),
                Some(start) if start > 0 => group_start(start - 1, self.letter(list[start - 1])),
                _ => None,
            }
        };
        if let Some(t) = target {
            self.game = t;
        }
        self.flash = Some((self.letter(list[self.game]), Instant::now()));
    }

    pub fn flash(&self) -> Option<char> {
        self.flash.filter(|(_, at)| at.elapsed() < FLASH_TTL).map(|(c, _)| c)
    }

    // ---- status ----

    pub fn set_status(&mut self, level: Level, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), level, at: Instant::now() });
    }

    pub fn visible_status(&self) -> Option<&Status> {
        self.status
            .as_ref()
            .filter(|s| s.at.elapsed() < STATUS_TTL || (s.level == Level::Error && s.at.elapsed() < STATUS_TTL * 2))
    }

    // ---- input ----

    fn input_blocked(&self) -> bool {
        self.running.is_some() || self.cooldown_until.is_some_and(|t| Instant::now() < t)
    }

    pub fn on_term_event(&mut self, ev: Event) {
        match ev {
            Event::FocusGained => self.term_focused = true,
            Event::FocusLost => self.term_focused = false,
            Event::Key(k) if k.kind != KeyEventKind::Release && !self.input_blocked() => {
                self.on_key(k);
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if ctrl && k.code == KeyCode::Char('r') {
            self.reload();
            return;
        }
        if self.filter_panel.is_some() {
            let op = match k.code {
                KeyCode::Up | KeyCode::Char('k') => Op::Up,
                KeyCode::Down | KeyCode::Char('j') => Op::Down,
                KeyCode::Left | KeyCode::Char('h') => Op::Left,
                KeyCode::Right | KeyCode::Char('l') => Op::Right,
                KeyCode::Enter => Op::Confirm,
                KeyCode::Esc | KeyCode::Char('q' | 'F') => Op::Back,
                _ => return,
            };
            self.apply(op);
            return;
        }
        if self.menu.is_some() {
            let op = match k.code {
                KeyCode::Up | KeyCode::Char('k') => Op::Up,
                KeyCode::Down | KeyCode::Char('j') => Op::Down,
                KeyCode::Enter => Op::Confirm,
                KeyCode::Esc | KeyCode::Char('q' | 'm') => Op::Back,
                _ => return,
            };
            self.apply(op);
            return;
        }

        if self.search.is_some() {
            let op = match k.code {
                KeyCode::Up => Some(Op::Up),
                KeyCode::Down => Some(Op::Down),
                KeyCode::Char('p') if ctrl => Some(Op::Up),
                KeyCode::Char('n') if ctrl => Some(Op::Down),
                KeyCode::PageUp => Some(Op::PageUp),
                KeyCode::PageDown => Some(Op::PageDown),
                KeyCode::Enter => Some(Op::Confirm),
                KeyCode::Esc => Some(Op::Back),
                KeyCode::Backspace => {
                    let s = self.search.as_mut().unwrap();
                    if s.query.pop().is_none() {
                        self.end_search();
                    } else {
                        self.update_search();
                    }
                    None
                }
                KeyCode::Char(c) if !ctrl => {
                    self.search.as_mut().unwrap().query.push(c);
                    self.update_search();
                    None
                }
                _ => None,
            };
            if let Some(op) = op {
                self.apply(op);
            }
            return;
        }

        let op = match k.code {
            KeyCode::Char('q') => {
                self.quit = true;
                return;
            }
            KeyCode::Char('/') => {
                self.start_search();
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => Op::Up,
            KeyCode::Down | KeyCode::Char('j') => Op::Down,
            KeyCode::Char('u') if ctrl => Op::PageUp,
            KeyCode::Char('d') if ctrl => Op::PageDown,
            KeyCode::PageUp => Op::PageUp,
            KeyCode::PageDown => Op::PageDown,
            KeyCode::Home | KeyCode::Char('g') => Op::Top,
            KeyCode::End | KeyCode::Char('G') => Op::Bottom,
            KeyCode::Left | KeyCode::Char('h') => Op::Left,
            KeyCode::Right | KeyCode::Char('l') => Op::Right,
            KeyCode::Enter => Op::Confirm,
            KeyCode::Esc | KeyCode::Backspace => Op::Back,
            KeyCode::Tab | KeyCode::Char(']') => Op::NextSystem,
            KeyCode::BackTab | KeyCode::Char('[') => Op::PrevSystem,
            KeyCode::Char('f') => Op::Favorite,
            KeyCode::Char('r') => Op::Random,
            KeyCode::Char('d') => Op::ToggleDetails,
            KeyCode::Char('m') => Op::Menu,
            KeyCode::Char('F') => Op::Filter,
            _ => return,
        };
        self.apply(op);
    }

    pub fn on_pad(&mut self, ev: PadEvent) {
        match ev {
            PadEvent::Connected(name) => self.pad = PadState::Connected(name),
            PadEvent::Disconnected => self.pad = PadState::Searching,
            PadEvent::Unavailable(e) => self.pad = PadState::Unavailable(e),
            PadEvent::Action(a) => {
                if self.input_blocked() || (self.cfg.controller.require_focus && !self.term_focused) {
                    return;
                }
                self.apply(match a {
                    PadAction::Up => Op::Up,
                    PadAction::Down => Op::Down,
                    PadAction::Left => Op::Left,
                    PadAction::Right => Op::Right,
                    PadAction::Confirm => Op::Confirm,
                    PadAction::Back => Op::Back,
                    PadAction::Favorite => Op::Favorite,
                    PadAction::Random => Op::Random,
                    PadAction::PrevSystem => Op::PrevSystem,
                    PadAction::NextSystem => Op::NextSystem,
                    PadAction::PrevLetter => Op::PrevLetter,
                    PadAction::NextLetter => Op::NextLetter,
                    PadAction::Menu => Op::Menu,
                    PadAction::Search => Op::Search,
                    PadAction::Filter => Op::Filter,
                });
            }
        }
    }

    fn apply(&mut self, op: Op) {
        if self.filter_panel.is_some() {
            return self.filter_op(op);
        }
        if self.menu.is_some() {
            return self.menu_op(op);
        }
        if self.osk.is_some() {
            return self.osk_op(op);
        }
        let page = self.page.max(1) as isize;
        match op {
            Op::Up => self.move_cursor(-1),
            Op::Down => self.move_cursor(1),
            Op::PageUp => self.move_cursor(-page),
            Op::PageDown => self.move_cursor(page),
            Op::Top => self.move_cursor(isize::MIN / 2),
            Op::Bottom => self.move_cursor(isize::MAX / 2),
            Op::Left => self.focus = Pane::Systems,
            Op::Right => self.focus = Pane::Games,
            Op::Confirm => match self.focus {
                Pane::Systems if self.search.is_none() => self.focus = Pane::Games,
                _ => {
                    if let Some(g) = self.selected_game() {
                        self.launch(g);
                    }
                }
            },
            Op::Back => {
                if self.search.is_some() {
                    self.end_search();
                } else {
                    self.focus = Pane::Systems;
                }
            }
            Op::NextSystem | Op::PrevSystem => {
                self.end_search();
                if !self.views.is_empty() {
                    let n = self.views.len();
                    let to = if matches!(op, Op::NextSystem) { self.view + 1 } else { self.view + n - 1 };
                    self.switch_view(to % n);
                }
            }
            Op::Favorite => {
                if let Some(g) = self.selected_game() {
                    let key = self.lib.games[g].key.clone();
                    let added = self.state.toggle_favorite(&key);
                    self.state.save();
                    let name = self.lib.games[g].name.clone();
                    self.set_status(
                        Level::Info,
                        if added { format!("★ {name}") } else { format!("Unfavorited {name}") },
                    );
                    if self.search.is_none() {
                        self.rebuild_keeping_selection();
                    }
                }
            }
            Op::Random => {
                let n = self.list().len();
                if n > 0 {
                    self.game = random_below(n);
                    self.focus = Pane::Games;
                }
            }
            Op::ToggleDetails => self.show_details = !self.show_details,
            Op::NextLetter => self.jump_letter(true),
            Op::PrevLetter => self.jump_letter(false),
            Op::Menu => self.menu = Some(0),
            Op::Filter => self.filter_panel = Some(0),
            Op::Search => {
                if self.search.is_none() {
                    self.start_search();
                }
                self.osk = Some((0, 0));
            }
        }
    }

    fn menu_op(&mut self, op: Op) {
        let Some(i) = self.menu else { return };
        match op {
            Op::Up => self.menu = Some((i + MENU.len() - 1) % MENU.len()),
            Op::Down => self.menu = Some((i + 1) % MENU.len()),
            Op::Back | Op::Menu => self.menu = None,
            Op::Confirm => {
                self.menu = None;
                match MENU[i] {
                    "Search" => self.apply(Op::Search),
                    "Filter" => self.apply(Op::Filter),
                    "Random game" => self.apply(Op::Random),
                    "Details pane" => self.apply(Op::ToggleDetails),
                    "Reload config" => self.reload(),
                    "Quit romi" => self.quit = true,
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// The filter panel: up/down picks a row, left/right (or A) changes it, and the list updates
    /// behind the panel as you go.
    fn filter_op(&mut self, op: Op) {
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

    /// Games matching the filter, out of all of them.
    pub fn filtered_count(&self) -> usize {
        self.lib.games.iter().filter(|g| self.filter.matches(g, &self.state)).count()
    }

    /// The on-screen keyboard: d-pad moves, A types, X deletes, Y adds a space, B or ✓ closes it
    /// (the results stay, ready to browse).
    fn osk_op(&mut self, op: Op) {
        let Some((row, col)) = self.osk else { return };
        let (rows, cols) = (OSK.len(), OSK[0].len());
        match op {
            Op::Up => self.osk = Some(((row + rows - 1) % rows, col)),
            Op::Down => self.osk = Some(((row + 1) % rows, col)),
            Op::Left => self.osk = Some((row, (col + cols - 1) % cols)),
            Op::Right => self.osk = Some((row, (col + 1) % cols)),
            Op::Random => self.edit_query(None),
            Op::Favorite => self.edit_query(Some(' ')),
            Op::Back | Op::Menu | Op::Search => self.close_osk(),
            Op::Confirm => match OSK[row][col] {
                "␣" => self.edit_query(Some(' ')),
                "⌫" => self.edit_query(None),
                "✓" => self.close_osk(),
                key => self.edit_query(key.chars().next().map(|c| c.to_ascii_lowercase())),
            },
            _ => {}
        }
    }

    /// Types a character into the search, or deletes one with `None`.
    fn edit_query(&mut self, c: Option<char>) {
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
        // Closing with nothing typed means there's nothing to browse.
        if self.search.as_ref().is_some_and(|s| s.query.trim().is_empty()) {
            self.end_search();
        }
    }

    // ---- search ----

    fn start_search(&mut self) {
        if let Some(v) = self.views.get(self.view) {
            self.positions.insert(v.id.clone(), self.game);
        }
        self.search = Some(Search { query: String::new(), results: Vec::new() });
        self.update_search();
        self.focus = Pane::Games;
    }

    fn end_search(&mut self) {
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
    fn update_search(&mut self) {
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
        results.retain(|&g| self.filter.matches(&self.lib.games[g], &self.state));
        if let Some(s) = &mut self.search {
            s.results = results;
        }
        self.game = 0;
        self.games_list = ListState::default();
    }

    // ---- launching ----

    fn launch(&mut self, g: usize) {
        let game = &self.lib.games[g];
        let name = game.name.clone();
        let prepared = match game.launch.prepare() {
            Ok(p) => p,
            Err(e) => {
                self.set_status(Level::Error, format!("{name}: {e:#}"));
                return;
            }
        };
        let result = match game.launch.mode {
            LaunchMode::Wait => prepared.spawn_quiet(&log_path()).map(|child| {
                self.running = Some(Running { game: g, child, started: Instant::now() });
            }),
            LaunchMode::Detach => prepared.spawn_detached(&log_path()).map(|()| {
                self.set_status(Level::Info, format!("Started {name}"));
            }),
            LaunchMode::Terminal => {
                self.terminal_job = Some(TerminalJob { game: g, prepared });
                Ok(())
            }
        };
        match result {
            Ok(()) => {
                let key = self.lib.games[g].key.clone();
                self.state.record_launch(&key);
                self.state.save();
            }
            Err(e) => self.set_status(Level::Error, format!("{e:#}")),
        }
    }

    /// Called every loop iteration: picks up decoded covers and notices when a game exits.
    pub fn tick(&mut self) {
        if let Some(c) = &mut self.covers {
            c.poll();
        }
        if let Some(found) = self.metadata.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.metadata = None;
            crate::meta::apply(&mut self.lib, found);
            self.filter_options = filter::Options::from(&self.lib);
            self.rebuild_keeping_selection();
        }
        let Some(run) = &mut self.running else { return };
        let status = match run.child.try_wait() {
            Ok(None) => return,
            Ok(Some(s)) => Some(s),
            Err(_) => None,
        };
        let run = self.running.take().unwrap();
        self.finish(run.game, run.started.elapsed(), status.map(|s| s.code()));
    }

    /// Bookkeeping after a game exits (wait or terminal mode).
    pub fn finish(&mut self, g: usize, played: Duration, code: Option<Option<i32>>) {
        let key = self.lib.games[g].key.clone();
        let name = self.lib.games[g].name.clone();
        self.state.add_playtime(&key, played.as_secs());
        self.state.save();
        // Whatever window had focus is gone; assume we're back in front.
        self.term_focused = true;
        self.cooldown_until = Some(Instant::now() + INPUT_COOLDOWN);
        match code {
            Some(Some(0)) => {
                self.set_status(Level::Info, format!("Played {name} for {}", crate::ui::fmt_duration(played.as_secs())))
            }
            Some(Some(c)) => {
                self.set_status(Level::Error, format!("{name} exited with code {c} (log: {})", tilde(&log_path())))
            }
            Some(None) => self.set_status(Level::Warn, format!("{name} was killed by a signal")),
            None => self.set_status(Level::Error, format!("Lost track of {name}")),
        }
        self.rebuild_keeping_selection();
    }

    pub fn reload(&mut self) {
        match config::load(&self.cfg_path) {
            Ok(cfg) => {
                self.lib = Library::load(&cfg);
                if let Some(c) = &mut self.covers {
                    c.reset(&cfg);
                }
                self.cfg = cfg;
                self.load_metadata();
                self.filter_options = filter::Options::from(&self.lib);
                self.end_search();
                self.rebuild_keeping_selection();
                self.set_status(Level::Info, format!("Reloaded: {} games", self.lib.games.len()));
            }
            Err(e) => self.set_status(Level::Error, format!("Config error: {e:#}")),
        }
    }

    pub fn save_selection(&mut self) {
        self.state.selected_system = self.views.get(self.view).map(|v| v.id.clone());
        self.state.selected_game = self.selected_game().map(|g| self.lib.games[g].key.clone());
        self.state.save();
    }
}

pub fn tilde(p: &std::path::Path) -> String {
    let home = config::home();
    match p.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

fn random_below(n: usize) -> usize {
    // No need for a crate: the clock's nanoseconds, scrambled a little, are plenty random here.
    let mut x =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x % n as u64) as usize
}
