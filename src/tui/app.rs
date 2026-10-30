//! App state and input handling. Keyboard and controller both funnel into the same `Op`s.

use crate::config::{self, Config, Emulator, LaunchMode};
use crate::emulators::Emulators;
use crate::filter::{self, Filter};
use crate::gamepad::{PadAction, PadEvent};
use crate::hooks::Hooks;
use crate::launch::Prepared;
use crate::library::Library;
use crate::session::Session;
use crate::sort::Sort;
use crate::state::State;
use crate::tui::cover::Covers;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

const STATUS_TTL: Duration = Duration::from_secs(6);
/// How long the big letter stays up after a letter jump.
const FLASH_TTL: Duration = Duration::from_millis(700);

/// An entry in a menu: the main one (Start), or the selected game's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    /// Opens the selected game's menu.
    Game,
    Search,
    Filter,
    Sort,
    Random,
    Details,
    Grid,
    Stats,
    Achievements,
    ShowHidden,
    /// Opens the options panel.
    Options,
    Reload,
    Quit,
    Favorite,
    Hide,
    Emulator,
    /// Plays the game inside romi, in the middle panel.
    PlayHere,
    /// Opens the game's tags.
    Tags,
    /// Opens the achievements screen on the game.
    GameAchievements,
    /// One of your tags (by its place in name order), ticked when the game has it.
    Tag(usize),
    NewTag,
}

/// An open menu. Everything that would otherwise need a keyboard is in one.
pub struct Menu {
    /// Whose menu: the selected game's, or None for the main menu.
    pub game: Option<usize>,
    pub selected: usize,
    /// The game's tags rather than its menu.
    pub tags: bool,
}

/// On-screen keyboard for searching with a controller.
pub const OSK: [[&str; 13]; 3] = [
    ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M"],
    ["N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z"],
    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "␣", "⌫", "✓"],
];
/// After a game exits, ignore input briefly so the button used to quit doesn't launch something.
const INPUT_COOLDOWN: Duration = Duration::from_millis(600);
/// How often the config's files are looked at for changes.
const WATCH_EVERY: Duration = Duration::from_millis(500);

/// When a file was changed and how big it is, or None if it's missing.
type Stamp = Option<(std::time::SystemTime, u64)>;

/// The config's files, and how they were when romi last read them, so it can reload when one
/// changes: by hand, from another romi, or from romi's own menus.
struct Watched {
    files: Vec<PathBuf>,
    seen: Vec<Stamp>,
    /// Changed files as they were at the last look, to wait for them to stop changing.
    settling: Option<Vec<Stamp>>,
    looked: Instant,
}

impl Watched {
    fn new(cfg_path: &Path, cfg: &Config) -> Watched {
        let files = config::files(cfg_path, cfg);
        let seen = stamps(&files);
        Watched { files, seen, settling: None, looked: Instant::now() }
    }

    /// True once the files have changed and then stayed the same between two looks, so a file an
    /// editor is still writing isn't read half done.
    fn changed(&mut self) -> bool {
        if self.looked.elapsed() < WATCH_EVERY {
            return false;
        }
        self.looked = Instant::now();
        let now = stamps(&self.files);
        if now == self.seen {
            self.settling = None;
            return false;
        }
        let settled = self.settling.as_ref() == Some(&now);
        self.settling = Some(now);
        settled
    }
}

fn stamps(files: &[PathBuf]) -> Vec<Stamp> {
    files.iter().map(|f| std::fs::metadata(f).ok().and_then(|m| Some((m.modified().ok()?, m.len())))).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Systems,
    Games,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewKind {
    Favorites,
    Recent,
    /// One of your tags, as a collection.
    Tag(String),
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
    pub session: Session,
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
    Sort,
    GameMenu,
    ToggleGrid,
    Stats,
    Achievements,
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
    /// A game played inside romi, in the middle panel.
    pub embedded: Option<crate::emu::embedded::Embedded>,
    /// The colors the launch animation picks from, and whether the terminal's background is light.
    pub look: crate::emu::wireframe::Look,
    /// What the terminal said its colors are, when romi started.
    pub terminal_colors: crate::emu::wireframe::TerminalColors,
    /// Whether B confirms and A goes back, for the controller's thread, which reads it on each press.
    pub swap_confirm: Arc<AtomicBool>,
    watched: Watched,
    /// Cores seen to start in a separate romi, which are safe to load into this one.
    cores_tried: HashSet<PathBuf>,
    pub pad: PadState,
    pub show_details: bool,
    pub terminal_job: Option<TerminalJob>,
    /// Box art, when enabled and the terminal could be queried.
    pub covers: Option<Covers>,
    pub menu: Option<Menu>,
    /// List hidden games too (dimmed), so they can be brought back.
    pub show_hidden: bool,
    /// A new tag's name while it's being typed, in the game's tags.
    pub tag_name: Option<String>,
    /// The other emulators for each system, found the first time one of its games' menus opens.
    emulators: Emulators,
    /// Cursor (row, column) while the on-screen keyboard is open.
    pub osk: Option<(usize, usize)>,
    /// Letter (or year) shown briefly after jumping to it.
    flash: Option<(String, Instant)>,
    pub filter: Filter,
    pub filter_options: filter::Options,
    /// Selected row while the filter panel is open.
    pub filter_panel: Option<usize>,
    /// The options panel, open on this row.
    pub options: Option<usize>,
    /// Database metadata being looked up in the background.
    metadata: Option<Receiver<Vec<crate::meta::Found>>>,
    pub quit: bool,
    pub systems_list: ListState,
    pub games_list: ListState,
    /// Games on one screen of the list or grid, set while drawing; used for paging.
    pub page: usize,
    /// Games per row of the cover grid, set while drawing; 0 in the list.
    pub grid_cols: usize,
    /// The grid's first visible row, kept between frames so it scrolls like the list.
    pub grid_top: usize,
    /// The game the details pane's pictures are for, and since when: they take turns from then.
    pub art_since: Option<(usize, Instant)>,
    /// The stats screen, while it's open.
    pub stats: Option<crate::stats::Stats>,
    /// The achievements screen, while it's open.
    pub cheevos: Option<crate::tui::cheevos::Screen>,
    /// What RetroAchievements has for the library: what was cached, then the refresh.
    pub achievements: crate::ra::Achievements,
    achievements_rx: Option<Receiver<Result<crate::ra::Achievements, String>>>,
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
        let watched = Watched::new(&cfg_path, &cfg);
        let swap_confirm = Arc::new(AtomicBool::new(cfg.controller.swap_confirm));
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
            embedded: None,
            look: crate::emu::wireframe::Look::new(None, Default::default()),
            terminal_colors: Default::default(),
            swap_confirm,
            watched,
            cores_tried: HashSet::new(),
            pad,
            terminal_job: None,
            covers: None,
            menu: None,
            show_hidden: false,
            tag_name: None,
            emulators: Emulators::default(),
            osk: None,
            flash: None,
            metadata: None,
            filter: Filter::default(),
            filter_options: filter::Options::default(),
            filter_panel: None,
            options: None,
            quit: false,
            systems_list: ListState::default(),
            games_list: ListState::default(),
            page: 10,
            grid_cols: 0,
            grid_top: 0,
            art_since: None,
            stats: None,
            cheevos: None,
            achievements: crate::ra::Achievements::default(),
            achievements_rx: None,
            term_focused: true,
            cooldown_until: None,
            positions: HashMap::new(),
            matcher: SkimMatcherV2::default(),
        };
        app.load_metadata();
        app.load_achievements();
        app.filter_options = filter::Options::from(&app.lib, &app.state);
        app.rebuild_views();
        let (mut sys, mut game) = (app.state.selected_system.clone(), app.state.selected_game.clone());
        if app.cfg.start_on == config::StartOn::LastPlayed
            && let Some(g) = app.state.recent.first().and_then(|k| app.lib.find(k))
        {
            sys = Some(app.lib.system_of(g).id.clone());
            game = Some(app.lib.games[g].key.clone());
        }
        app.select(sys.as_deref(), game.as_deref());
        for w in std::mem::take(&mut app.lib.warnings) {
            app.set_status(Level::Warn, w);
        }
        app
    }

    /// Shows the cached RetroAchievements data, and refreshes it off the UI thread when
    /// RetroAchievements is set up (`tick` picks the result up).
    fn load_achievements(&mut self) {
        self.achievements = crate::ra::Achievements::cached(&self.cfg, &self.lib);
        if crate::ra::credentials(&self.cfg).is_none() {
            self.achievements_rx = None;
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (cfg, lib) = (self.cfg.clone(), self.lib.clone());
        std::thread::Builder::new()
            .name("achievements".into())
            .spawn(move || {
                let _ = tx.send(crate::ra::Achievements::refresh(&cfg, &lib).map_err(|e| format!("{e:#}")));
            })
            .ok();
        self.achievements_rx = Some(rx);
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
        let keep = |g: &usize| self.visible(*g) && self.filter.matches(&self.lib.games[*g], &self.state);
        let resolve = |keys: &[String]| keys.iter().filter_map(|k| self.lib.find(k)).filter(keep).collect::<Vec<_>>();
        let mut views = Vec::new();
        let mut favs = resolve(&self.state.favorites);
        self.sort_for("@favorites", true).apply(&self.lib, &self.state, &mut favs);
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
        for (tag, keys) in &self.state.tags {
            let id = format!("#{tag}");
            let mut games = resolve(keys);
            self.sort_for(&id, true).apply(&self.lib, &self.state, &mut games);
            if !games.is_empty() {
                views.push(View { kind: ViewKind::Tag(tag.clone()), id, name: tag.clone(), games });
            }
        }
        for (i, s) in self.lib.systems.iter().enumerate() {
            let mut games: Vec<usize> = s.games.iter().copied().filter(keep).collect();
            self.sort_for(&s.id, false).apply(&self.lib, &self.state, &mut games);
            // While filtering, systems with nothing left would just be noise.
            if !games.is_empty() || !self.filter.is_active() {
                views.push(View { kind: ViewKind::System(i), id: s.id.clone(), name: s.name.clone(), games });
            }
        }
        self.views = views;
    }

    fn sort_for(&self, view_id: &str, favorites: bool) -> Sort {
        self.state.sort.get(view_id).copied().unwrap_or(Sort::options(favorites)[0])
    }

    /// How the games pane is sorted; None for Recent and search results, which have their own order.
    pub fn sort(&self) -> Option<Sort> {
        let view = self.views.get(self.view).filter(|_| self.search.is_none())?;
        match view.kind {
            ViewKind::Recent => None,
            ViewKind::Favorites | ViewKind::Tag(_) => Some(self.sort_for(&view.id, true)),
            ViewKind::System(_) => Some(self.sort_for(&view.id, false)),
        }
    }

    /// Switches the current view to its next sort order, and remembers it.
    fn cycle_sort(&mut self) {
        let Some(view) = self.views.get(self.view) else { return };
        let favorites = matches!(view.kind, ViewKind::Favorites | ViewKind::Tag(_));
        if view.kind == ViewKind::Recent {
            self.set_status(Level::Info, "Recent is always newest first");
            return;
        }
        let (id, next) = (view.id.clone(), self.sort_for(&view.id, favorites).next(favorites));
        self.state = State::update(|s| {
            if next == Sort::options(favorites)[0] {
                s.sort.remove(&id);
            } else {
                s.sort.insert(id, next);
            }
        });
        if self.search.is_none() {
            self.rebuild_keeping_selection();
        }
        self.set_status(Level::Info, format!("Sorted by {}", next.label()));
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

    /// The section a game is filed under: its year when sorted by year, otherwise the first
    /// letter ignoring "The", or `#` for anything else.
    fn letter(&self, g: usize) -> String {
        if self.sort() == Some(Sort::Year) {
            return self.lib.games[g].meta.year.map_or("?".into(), |y| y.to_string());
        }
        match self.lib.games[g].sort_key.chars().find(|c| c.is_alphanumeric()) {
            Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string(),
            _ => "#".into(),
        }
    }

    /// Moves to the first game of the next letter, or back to the start of this one (then the
    /// previous letter), like a phone's contact list. Lists sorted by play have no sections, so
    /// this pages through them instead.
    fn jump_letter(&mut self, forward: bool) {
        let list = self.list().to_vec();
        if list.is_empty() {
            return;
        }
        self.focus = Pane::Games;
        if self.sort().is_some_and(|s| !s.has_sections()) {
            let page = self.page.max(1) as isize;
            return self.move_cursor(if forward { page } else { -page });
        }
        let here = self.letter(list[self.game]);
        let group_start = |end: usize, l: &str| (0..=end).rev().take_while(|&i| self.letter(list[i]) == l).last();
        let target = if forward {
            (self.game + 1..list.len()).find(|&i| self.letter(list[i]) != here)
        } else {
            match group_start(self.game, &here) {
                Some(start) if start < self.game => Some(start),
                Some(start) if start > 0 => group_start(start - 1, &self.letter(list[start - 1])),
                _ => None,
            }
        };
        if let Some(t) = target {
            self.game = t;
        }
        self.flash = Some((self.letter(list[self.game]), Instant::now()));
    }

    pub fn flash(&self) -> Option<&str> {
        self.flash.as_ref().filter(|(_, at)| at.elapsed() < FLASH_TTL).map(|(s, _)| s.as_str())
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

    /// Picks up favorites and play history recorded by other romi processes, like `romi play`.
    fn reload_state(&mut self) {
        let state = State::load();
        if state != self.state {
            self.state = state;
            self.rebuild_keeping_selection();
        }
    }

    fn input_blocked(&self) -> bool {
        self.running.is_some() || self.cooldown_until.is_some_and(|t| Instant::now() < t)
    }

    pub fn on_term_event(&mut self, ev: Event) {
        // A game in the middle panel gets every key, releases too.
        if let (Some(embedded), Event::Key(k)) = (&mut self.embedded, &ev) {
            if !embedded.on_key(*k) {
                self.stop_embedded();
            }
            return;
        }
        match ev {
            Event::FocusGained => {
                self.term_focused = true;
                self.reload_state();
            }
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
        if let Some(name) = &mut self.tag_name {
            match k.code {
                KeyCode::Enter => self.save_tag_name(),
                KeyCode::Esc => self.cancel_tag_name(),
                KeyCode::Backspace => {
                    name.pop();
                }
                KeyCode::Char(c) if !ctrl => name.push(c),
                _ => {}
            }
            return;
        }
        if self.stats.is_some() {
            if matches!(k.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Backspace | KeyCode::Char('q' | 'S' | 'm')) {
                self.stats = None;
            }
            return;
        }
        if let Some(screen) = &mut self.cheevos {
            if screen.typing {
                match k.code {
                    KeyCode::Enter | KeyCode::Esc => {
                        screen.typing = false;
                        self.osk = None;
                    }
                    KeyCode::Backspace => {
                        screen.query.pop();
                        screen.selected = 0;
                    }
                    KeyCode::Char(c) if !ctrl => {
                        screen.query.push(c);
                        screen.selected = 0;
                    }
                    _ => {}
                }
                return;
            }
            let op = match k.code {
                KeyCode::Up | KeyCode::Char('k') => Op::Up,
                KeyCode::Down | KeyCode::Char('j') => Op::Down,
                KeyCode::PageUp => Op::PageUp,
                KeyCode::PageDown => Op::PageDown,
                KeyCode::Char('u') if ctrl => Op::PageUp,
                KeyCode::Char('d') if ctrl => Op::PageDown,
                KeyCode::Left | KeyCode::Char('h' | '[') => Op::PrevSystem,
                KeyCode::Right | KeyCode::Char('l' | ']') => Op::NextSystem,
                KeyCode::Tab => Op::Random,
                KeyCode::BackTab => Op::Favorite,
                KeyCode::Char('/') => Op::Search,
                KeyCode::Enter => Op::Confirm,
                KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q' | 'A' | 'm') => Op::Back,
                _ => return,
            };
            self.cheevos_op(op);
            return;
        }
        if self.filter_panel.is_some() || self.options.is_some() {
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
                KeyCode::Esc | KeyCode::Char('q' | 'm' | 'o') => Op::Back,
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
            KeyCode::Char('s') => Op::Sort,
            KeyCode::Char('o') => Op::GameMenu,
            KeyCode::Char('v') => Op::ToggleGrid,
            KeyCode::Char('S') => Op::Stats,
            KeyCode::Char('A') => Op::Achievements,
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
                // A game in the middle panel reads the controller itself.
                if self.embedded.is_some()
                    || self.input_blocked()
                    || (self.cfg.controller.require_focus && !self.term_focused)
                {
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
                    PadAction::ToggleGrid => Op::ToggleGrid,
                });
            }
        }
    }

    fn apply(&mut self, op: Op) {
        // The on-screen keyboard is up while a tag is named, over the game's tags.
        if self.tag_name.is_some() {
            return self.osk_op(op);
        }
        if self.stats.is_some() {
            if matches!(op, Op::Back | Op::Menu | Op::Confirm) {
                self.stats = None;
            }
            return;
        }
        if self.cheevos.is_some() {
            if self.osk.is_some() {
                return self.osk_op(op);
            }
            return self.cheevos_op(op);
        }
        if self.filter_panel.is_some() {
            return self.filter_op(op);
        }
        if self.options.is_some() {
            return self.options_op(op);
        }
        if self.menu.is_some() {
            return self.menu_op(op);
        }
        if self.osk.is_some() {
            return self.osk_op(op);
        }
        let page = self.page.max(1) as isize;
        // In the cover grid, up and down go a row at a time, and left and right a cover, until
        // left runs out of row and goes to the systems, as in the list.
        let grid = self.grid_cols.max(1) as isize;
        let in_grid = self.state.grid && self.grid_cols > 0 && (self.focus == Pane::Games || self.search.is_some());
        match op {
            Op::Up if in_grid => self.move_cursor(-grid),
            Op::Down if in_grid => self.move_cursor(grid),
            Op::Left if in_grid && !self.game.is_multiple_of(self.grid_cols) => self.move_cursor(-1),
            Op::Right if in_grid => self.move_cursor(1),
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
                    self.toggle_favorite(g);
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
            Op::Achievements => self.cheevos = Some(crate::tui::cheevos::Screen::default()),
            Op::Stats => {
                let (history, now) = (crate::stats::history(), crate::state::now());
                self.stats = Some(crate::stats::Stats::compute(&self.lib, &self.state, &history, now));
            }
            Op::ToggleGrid => {
                let grid = !self.state.grid;
                self.state = State::update(|s| s.grid = grid);
                self.grid_top = 0;
            }
            Op::NextLetter => self.jump_letter(true),
            Op::PrevLetter => self.jump_letter(false),
            Op::Menu => self.menu = Some(Menu { game: None, selected: 0, tags: false }),
            Op::GameMenu => {
                if let Some(g) = self.selected_game() {
                    self.emulators.find(&self.cfg, &self.lib, g);
                    self.menu = Some(Menu { game: Some(g), selected: 0, tags: false });
                }
            }
            Op::Filter => self.filter_panel = Some(0),
            Op::Sort => self.cycle_sort(),
            Op::Search => {
                if self.search.is_none() {
                    self.start_search();
                }
                self.osk = Some((0, 0));
            }
        }
    }

    fn toggle_favorite(&mut self, g: usize) {
        let key = self.lib.games[g].key.clone();
        let mut added = false;
        self.state = State::update(|s| added = s.toggle_favorite(&key));
        let name = self.lib.games[g].name.clone();
        self.set_status(Level::Info, if added { format!("★ {name}") } else { format!("Unfavorited {name}") });
        if self.search.is_none() {
            self.rebuild_keeping_selection();
        }
    }

    /// The achievements screen: the list of games, or one game's achievements.
    fn cheevos_op(&mut self, op: Op) {
        let page = self.page.max(1) as isize;
        let Some(screen) = &mut self.cheevos else { return };
        if let Some(open) = &mut screen.open {
            let n = open.cheevos.as_ref().map_or(0, Vec::len) as isize;
            let scroll = |top: usize, by: isize| (top as isize + by).clamp(0, (n - 1).max(0)) as usize;
            match op {
                Op::Up => open.top = scroll(open.top, -1),
                Op::Down => open.top = scroll(open.top, 1),
                Op::PageUp => open.top = scroll(open.top, -page / 2),
                Op::PageDown => open.top = scroll(open.top, page / 2),
                Op::Back | Op::Menu | Op::Confirm => {
                    screen.open = None;
                    if screen.just_one || matches!(op, Op::Menu) {
                        self.cheevos = None;
                    }
                }
                _ => {}
            }
            return;
        }
        let rows = screen.rows(&self.lib, &self.achievements, self.show_hidden);
        let n = rows.len() as isize;
        let move_to = |at: usize, by: isize| (at as isize + by).clamp(0, (n - 1).max(0)) as usize;
        match op {
            Op::Up => screen.selected = move_to(screen.selected, -1),
            Op::Down => screen.selected = move_to(screen.selected, 1),
            Op::PageUp => screen.selected = move_to(screen.selected, -page),
            Op::PageDown => screen.selected = move_to(screen.selected, page),
            Op::Left | Op::PrevSystem => screen.step_system(&self.lib, &self.achievements, false),
            Op::Right | Op::NextSystem => screen.step_system(&self.lib, &self.achievements, true),
            Op::Random | Op::Favorite => {
                screen.status = screen.status.step(matches!(op, Op::Random));
                screen.selected = 0;
            }
            Op::Search => {
                screen.typing = true;
                self.osk = Some((0, 0));
            }
            Op::Confirm => {
                if let Some(&g) = rows.get(screen.selected) {
                    screen.open_game(&self.cfg, &self.achievements, &self.lib, g);
                }
            }
            Op::Back | Op::Menu => self.cheevos = None,
            _ => {}
        }
    }

    /// Tags or untags a game. Tags live in state.json, and each shows as a collection.
    fn toggle_tag(&mut self, g: usize, tag: &str) {
        let key = self.lib.games[g].key.clone();
        let mut added = false;
        self.state = State::update(|s| added = s.toggle_tag(&key, tag));
        self.filter_options = filter::Options::from(&self.lib, &self.state);
        let name = &self.lib.games[g].name;
        self.set_status(Level::Info, if added { format!("{name}: #{tag}") } else { format!("{name}: no #{tag}") });
        if self.search.is_none() {
            self.rebuild_keeping_selection();
        }
    }

    /// Hides a game (or brings it back), saved in overrides.jsonc. The cursor stays where it was,
    /// on whatever moved into the hidden game's place.
    fn toggle_hidden(&mut self, g: usize) {
        let (key, name, hide) = {
            let game = &self.lib.games[g];
            (game.key.clone(), game.name.clone(), !game.hidden)
        };
        if let Err(e) = crate::overrides::update(&self.cfg_path, &key, |ov| ov.hidden = hide) {
            self.set_status(Level::Error, format!("Couldn't save: {e:#}"));
            return;
        }
        self.lib.games[g].hidden = hide;
        self.cfg.overrides.entry(key).or_default().hidden = hide;
        if self.search.is_some() {
            self.update_search();
        } else {
            let (view, at) = (self.views.get(self.view).map(|v| v.id.clone()), self.game);
            self.rebuild_views();
            self.select(view.as_deref(), None);
            self.game = at.min(self.list().len().saturating_sub(1));
        }
        let text = if !hide {
            format!("Unhid {name}")
        } else if self.show_hidden {
            format!("Hid {name}")
        } else {
            format!("Hid {name} (Show hidden games in the menu brings it back)")
        };
        self.set_status(Level::Info, text);
    }

    /// The ways a game can run, default first, once its menu has looked them up.
    pub fn emulator_options(&self, g: usize) -> Vec<Emulator> {
        self.emulators.options(&self.lib, g)
    }

    fn current_emulator(&self, g: usize, options: &[Emulator]) -> usize {
        crate::emulators::current(&self.lib.games[g], options)
    }

    /// Switches a game to the next way it can run, saved in overrides.jsonc.
    fn cycle_emulator(&mut self, g: usize) {
        let options = self.emulator_options(g);
        if options.len() < 2 {
            return;
        }
        let next = (self.current_emulator(g, &options) + 1) % options.len();
        let pick = &options[next];
        let key = self.lib.games[g].key.clone();
        let saved = crate::overrides::update(&self.cfg_path, &key, |ov| {
            let picked = next > 0;
            ov.emulator = picked.then(|| pick.name.clone());
            ov.command = picked.then(|| pick.command.clone());
            ov.mode = pick.mode.filter(|_| picked);
            ov.cwd = pick.cwd.clone().filter(|_| picked);
        });
        if let Err(e) = saved {
            self.set_status(Level::Error, format!("Couldn't save: {e:#}"));
            return;
        }
        // The same as loading the override would do: what it doesn't say stays the default's.
        let game = &mut self.lib.games[g];
        let default = game.default_run.clone();
        game.launch.command.clone_from(&pick.command);
        game.launch.mode = pick.mode.or(default.mode).unwrap_or_default();
        game.launch.cwd = pick.cwd.clone().or(default.cwd);
        game.emulator = (next > 0).then(|| pick.name.clone());
        let text = format!("{} runs with {}", game.name, pick.name);
        self.set_status(Level::Info, text);
    }

    /// Whether a game shows in lists and search.
    fn visible(&self, g: usize) -> bool {
        self.show_hidden || !self.lib.games[g].hidden
    }

    /// What the open menu offers, in order.
    pub fn menu_items(&self) -> Vec<MenuItem> {
        use MenuItem::*;
        let menu = self.menu.as_ref();
        if menu.is_some_and(|m| m.tags) {
            return (0..self.state.tags.len()).map(Tag).chain([NewTag]).collect();
        }
        match menu.and_then(|m| m.game) {
            Some(g) => {
                let mut items = Vec::new();
                if self.embeddable(g).is_some() {
                    items.push(PlayHere);
                }
                items.extend([Favorite, Hide, Tags]);
                if self.emulator_options(g).len() > 1 {
                    items.push(Emulator);
                }
                if self.achievements.games.contains_key(&self.lib.games[g].key) {
                    items.push(GameAchievements);
                }
                items
            }
            None => {
                let mut items = Vec::new();
                if self.selected_game().is_some() {
                    items.push(Game);
                }
                items.extend([Search, Filter, Sort, Random, Grid, Details, Stats]);
                if !self.achievements.games.is_empty() {
                    items.push(Achievements);
                }
                if self.lib.games.iter().any(|g| g.hidden) {
                    items.push(ShowHidden);
                }
                items.extend([Options, Reload, Quit]);
                items
            }
        }
    }

    pub fn menu_label(&self, item: MenuItem) -> String {
        let game = self.menu.as_ref().and_then(|m| m.game).or(self.selected_game()).map(|g| &self.lib.games[g]);
        match item {
            MenuItem::Game => format!("{}…", game.map_or("Game", |g| g.name.as_str())),
            MenuItem::Search => "Search".into(),
            MenuItem::Filter => "Filter".into(),
            MenuItem::Sort => match self.sort() {
                Some(sort) => format!("Sort: {}", sort.label()),
                None => "Sort: newest first".into(),
            },
            MenuItem::Random => "Random game".into(),
            MenuItem::Stats => "Stats".into(),
            MenuItem::Achievements => "Achievements".into(),
            MenuItem::GameAchievements => "Achievements…".into(),
            MenuItem::Grid if self.state.grid => "List view".into(),
            MenuItem::Grid => "Cover grid".into(),
            MenuItem::Details if self.show_details => "Hide details".into(),
            MenuItem::Details => "Show details".into(),
            MenuItem::ShowHidden if self.show_hidden => "Hide hidden games".into(),
            MenuItem::ShowHidden => {
                format!("Show hidden games ({})", self.lib.games.iter().filter(|g| g.hidden).count())
            }
            MenuItem::Options => "Options…".into(),
            MenuItem::Reload => "Reload config".into(),
            MenuItem::PlayHere => "Play in romi".into(),
            MenuItem::Quit => "Quit romi".into(),
            MenuItem::Favorite if game.is_some_and(|g| self.state.is_favorite(&g.key)) => "Unfavorite".into(),
            MenuItem::Favorite => "Favorite".into(),
            MenuItem::Hide if game.is_some_and(|g| g.hidden) => "Unhide".into(),
            MenuItem::Hide => "Hide".into(),
            MenuItem::Tags => match game.map(|g| self.state.tags_of(&g.key)) {
                Some(tags) if !tags.is_empty() => format!("Tags: {}", tags.join(", ")),
                _ => "Tags…".into(),
            },
            MenuItem::Tag(i) => {
                let tag = self.state.tags.keys().nth(i).map_or("", String::as_str);
                let on = game.is_some_and(|g| self.state.has_tag(&g.key, tag));
                format!("{} {tag}", if on { "✓" } else { " " })
            }
            MenuItem::NewTag => match &self.tag_name {
                Some(name) => format!("+ {name}█"),
                None => "+ New tag…".into(),
            },
            MenuItem::Emulator => {
                let g = self.menu.as_ref().and_then(|m| m.game);
                let options = g.map(|g| self.emulator_options(g)).unwrap_or_default();
                let at = g.map_or(0, |g| self.current_emulator(g, &options));
                format!("Emulator: {}", options.get(at).map_or("default", |e| e.name.as_str()))
            }
        }
    }

    fn menu_op(&mut self, op: Op) {
        let items = self.menu_items();
        let Some(menu) = &mut self.menu else { return };
        let n = items.len().max(1);
        match op {
            Op::Up => menu.selected = (menu.selected + n - 1) % n,
            Op::Down => menu.selected = (menu.selected + 1) % n,
            // From the tags, back to the game's menu.
            Op::Back if menu.tags => {
                menu.tags = false;
                let at = self.menu_items().iter().position(|&i| i == MenuItem::Tags).unwrap_or(0);
                if let Some(menu) = &mut self.menu {
                    menu.selected = at;
                }
            }
            Op::Back | Op::Menu => self.menu = None,
            Op::Confirm => {
                let (game, selected, tags) = (menu.game, menu.selected, menu.tags);
                self.menu = None;
                let Some(&item) = items.get(selected) else { return };
                match item {
                    MenuItem::Game => self.apply(Op::GameMenu),
                    MenuItem::PlayHere => {
                        if let Some(g) = game {
                            self.play_here(g);
                        }
                    }
                    MenuItem::Search => self.apply(Op::Search),
                    MenuItem::Filter => self.apply(Op::Filter),
                    // Stays open, so A again tries the next order.
                    MenuItem::Sort => {
                        self.cycle_sort();
                        self.menu = Some(Menu { game, selected, tags: false });
                    }
                    MenuItem::Random => self.apply(Op::Random),
                    MenuItem::Grid => self.apply(Op::ToggleGrid),
                    MenuItem::Stats => self.apply(Op::Stats),
                    MenuItem::Achievements => self.apply(Op::Achievements),
                    MenuItem::GameAchievements => {
                        if let Some(g) = game {
                            self.cheevos = Some(crate::tui::cheevos::Screen::for_game(
                                &self.cfg,
                                &self.achievements,
                                &self.lib,
                                g,
                            ));
                        }
                    }
                    MenuItem::Details => self.apply(Op::ToggleDetails),
                    MenuItem::ShowHidden => {
                        self.show_hidden = !self.show_hidden;
                        self.rebuild_keeping_selection();
                    }
                    MenuItem::Options => self.options = Some(0),
                    MenuItem::Reload => self.reload(),
                    MenuItem::Quit => self.quit = true,
                    MenuItem::Favorite => {
                        if let Some(g) = game {
                            self.toggle_favorite(g);
                        }
                    }
                    MenuItem::Hide => {
                        if let Some(g) = game {
                            self.toggle_hidden(g);
                        }
                    }
                    MenuItem::Tags => self.menu = Some(Menu { game, selected: 0, tags: true }),
                    MenuItem::Tag(i) => {
                        if let (Some(g), Some(tag)) = (game, self.state.tags.keys().nth(i).cloned()) {
                            self.toggle_tag(g, &tag);
                            // Untagging the last game removes the tag, and the row with it.
                            let n = self.state.tags.len() + 1;
                            self.menu = Some(Menu { game, selected: selected.min(n - 1), tags });
                        }
                    }
                    MenuItem::NewTag => {
                        self.menu = Some(Menu { game, selected, tags });
                        self.tag_name = Some(String::new());
                        self.osk = Some((0, 0));
                    }
                    // Stays open, like Sort.
                    MenuItem::Emulator => {
                        if let Some(g) = game {
                            self.cycle_emulator(g);
                        }
                        self.menu = Some(Menu { game, selected, tags: false });
                        // An emulator with a libretro core adds Play in romi above, or takes it away.
                        if let Some(at) = self.menu_items().iter().position(|&i| i == MenuItem::Emulator)
                            && let Some(menu) = &mut self.menu
                        {
                            menu.selected = at;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The filter panel: up/down picks a row, left/right (or A) changes it, and the list updates
    /// behind the panel as you go.
    /// The options panel: up/down picks a setting, left/right (or A) changes it, saving it into
    /// the config straight away.
    fn options_op(&mut self, op: Op) {
        use crate::tui::options::SETTINGS;
        let Some(row) = self.options else { return };
        let n = SETTINGS.len();
        match op {
            Op::Up => self.options = Some((row + n - 1) % n),
            Op::Down => self.options = Some((row + 1) % n),
            Op::Back | Op::Menu => self.options = None,
            Op::Left => self.change_setting(SETTINGS[row], false),
            Op::Right | Op::Confirm => self.change_setting(SETTINGS[row], true),
            _ => {}
        }
    }

    /// Steps a setting to its next value and saves it into the config, changing only that value.
    fn change_setting(&mut self, setting: crate::tui::options::Setting, forward: bool) {
        let value = setting.step(&self.cfg, forward);
        // Changed by hand since romi last read it: everything has to be read again, not just this.
        let edited_elsewhere = stamps(&self.watched.files) != self.watched.seen;
        if let Err(e) = crate::config_edit::save(&self.cfg_path, setting.path(), &value) {
            return self.set_status(Level::Error, format!("{e:#}"));
        }
        if edited_elsewhere {
            return self.reload_as("Saved, and reloaded");
        }
        match config::load(&self.cfg_path) {
            Ok(cfg) => {
                let art_changed = (cfg.art.show, cfg.art.screens) != (self.cfg.art.show, self.cfg.art.screens);
                if let Some(c) = self.covers.as_mut().filter(|_| art_changed) {
                    c.reset(&cfg);
                }
                self.cfg = cfg;
                self.apply_settings();
                self.watched = Watched::new(&self.cfg_path, &self.cfg);
                let shown = setting.shown(&self.cfg);
                self.set_status(Level::Info, format!("{}: {shown}", setting.label()));
            }
            Err(e) => self.set_status(Level::Error, format!("Config error: {e:#}")),
        }
    }

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
            Op::Back | Op::Menu | Op::Search if self.tag_name.is_some() => self.cancel_tag_name(),
            Op::Back | Op::Menu | Op::Search => self.close_osk(),
            Op::Confirm => match OSK[row][col] {
                "␣" => self.edit_query(Some(' ')),
                "⌫" => self.edit_query(None),
                "✓" if self.tag_name.is_some() => self.save_tag_name(),
                "✓" => self.close_osk(),
                key => self.edit_query(key.chars().next().map(|c| c.to_ascii_lowercase())),
            },
            _ => {}
        }
    }

    /// Types a character into the search (or a new tag's name), or deletes one with `None`.
    fn edit_query(&mut self, c: Option<char>) {
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

    /// Tags the menu's game with the name typed (an existing tag if it matches one in any case),
    /// and leaves the cursor on it.
    fn save_tag_name(&mut self) {
        let (Some(name), Some(g)) = (self.tag_name.take(), self.menu.as_ref().and_then(|m| m.game)) else { return };
        self.osk = None;
        let tag = self.state.tag_named(&name);
        if tag.is_empty() {
            return;
        }
        if !self.state.has_tag(&self.lib.games[g].key, &tag) {
            self.toggle_tag(g, &tag);
        }
        if let Some(menu) = &mut self.menu {
            menu.selected = self.state.tags.keys().position(|t| *t == tag).unwrap_or(0);
        }
    }

    fn cancel_tag_name(&mut self) {
        self.tag_name = None;
        self.osk = None;
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
        results.retain(|&g| self.visible(g) && self.filter.matches(&self.lib.games[g], &self.state));
        if let Some(s) = &mut self.search {
            s.results = results;
        }
        self.game = 0;
        self.games_list = ListState::default();
    }

    // ---- launching ----

    /// The core and ROM to play a game inside romi with, if it runs through a libretro core.
    fn embeddable(&self, g: usize) -> Option<(PathBuf, PathBuf)> {
        let game = &self.lib.games[g];
        let rom = game.path.clone()?;
        let core = crate::emu::libretro::core_in(&game.launch.prepare().ok()?.argv)?;
        (core.is_file() && crate::emu::libretro::can_open(&core, &rom)).then_some((core, rom))
    }

    fn play_here(&mut self, g: usize) {
        let Some((core, rom)) = self.embeddable(g) else { return };
        if let Err(e) = self.try_core(&core, &rom) {
            return self.set_status(Level::Error, format!("{}: {e}", self.lib.games[g].name));
        }
        let volume = self.cfg.volume_gain();
        let launch = self.launch_animation(g, &rom);
        let started = crate::emu::runner::load(&self.cfg, &core, &rom)
            .and_then(|core| crate::emu::embedded::Embedded::start(g, core, volume, launch, self.cfg.corners()));
        match started {
            Ok(e) => self.embedded = Some(e),
            Err(e) => self.set_status(Level::Error, format!("{}: {e:#}", self.lib.games[g].name)),
        }
    }

    /// The launch animation for a game played here, if it's on: its system's console, or a TV.
    /// The ROM's file name seeds it, so a game always starts the same way.
    fn launch_animation(&self, g: usize, rom: &Path) -> Option<crate::emu::embedded::Launch> {
        use crate::emu::wireframe;
        if !self.cfg.launch_animation.enabled {
            return None;
        }
        let model = wireframe::Model::load(wireframe::for_system(&self.lib.system_of(g).id)).ok()?;
        let seed = wireframe::fnv1a(&rom.file_name()?.to_string_lossy());
        Some(crate::emu::embedded::Launch { model, seed, beam: self.look.beam(seed) })
    }

    /// Starts the core in a separate romi for a frame, the first time it's played here. A core
    /// that wants what romi can't give it can crash while loading (Flycast, which only draws
    /// with OpenGL or Vulkan, does), and in this process that would take the TUI down with it.
    fn try_core(&mut self, core: &Path, rom: &Path) -> Result<(), String> {
        if self.cores_tried.contains(core) {
            return Ok(());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let out = std::process::Command::new(exe)
            .arg("run")
            .arg(rom)
            .arg("--core")
            .arg(core)
            .args(["--frames", "1"])
            .env("ROMI_CONFIG", &self.cfg_path)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| e.to_string())?;
        match out.status.code() {
            Some(0) => {
                self.cores_tried.insert(core.to_path_buf());
                Ok(())
            }
            Some(_) => {
                let err = String::from_utf8_lossy(&out.stderr);
                let last = err.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("the core didn't start");
                Err(last.trim_start_matches("romi: ").to_string())
            }
            None => {
                Err("the core crashed while starting; it may only draw with OpenGL or Vulkan, which romi can't".into())
            }
        }
    }

    /// Stops the game in the middle panel. Input waits a moment, so the buttons that stopped it
    /// don't also do something in the list.
    pub fn stop_embedded(&mut self) {
        if let Some(e) = self.embedded.take().and_then(|e| e.stop()) {
            self.set_status(Level::Error, format!("Not saved: {e}"));
        }
        self.cooldown_until = Some(Instant::now() + INPUT_COOLDOWN);
    }

    /// Feeds the game in the middle panel: controller input in, the newest frame out.
    pub fn pump_embedded(&mut self) {
        let Some(embedded) = &mut self.embedded else { return };
        if !embedded.read_pads() {
            return self.stop_embedded();
        }
        if let Err(e) = embedded.pump() {
            self.stop_embedded();
            return self.set_status(Level::Error, format!("Stopped: {e:#}"));
        }
        let (sound_error, save_error) = (embedded.sound_error(), embedded.save_error());
        if let Some(e) = sound_error {
            self.set_status(Level::Warn, format!("No sound: {e}"));
        }
        if let Some(e) = save_error {
            self.set_status(Level::Error, format!("Not saved: {e}"));
        }
    }

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
            LaunchMode::Wait => {
                let hooks = Hooks::for_game(&self.cfg, &self.lib, g);
                let problem = hooks.launch();
                let started = prepared.spawn_logged(&log_path(), true).map(|child| {
                    let (session, state) = Session::begin(&self.lib, g, child.id(), hooks);
                    self.state = state;
                    self.running = Some(Running { game: g, child, started: Instant::now(), session });
                });
                if let Some(problem) = problem {
                    self.set_status(Level::Warn, problem);
                }
                started
            }
            // A copy of romi plays it in the background and records the session, so play time
            // counts even if this one quits first.
            LaunchMode::Detach => prepared
                .check_program()
                .and_then(|()| crate::session::play_in_background(&self.cfg_path, &game.key))
                .map(|mut watcher| {
                    std::thread::spawn(move || watcher.wait());
                    self.set_status(Level::Info, format!("Started {name}"));
                }),
            // The main loop releases the terminal, then starts it (see `run_in_terminal`).
            LaunchMode::Terminal => {
                self.terminal_job = Some(TerminalJob { game: g, prepared });
                Ok(())
            }
        };
        if let Err(e) = result {
            self.set_status(Level::Error, format!("{e:#}"));
        }
    }

    /// Called every loop iteration: picks up decoded covers and notices when a game exits.
    pub fn tick(&mut self) {
        if let Some(c) = &mut self.covers {
            c.poll();
        }
        // Not while a game runs: the game list it's in would be rebuilt under it.
        if self.embedded.is_none() && self.running.is_none() && self.watched.changed() {
            self.reload_as("Config changed, reloaded");
        }
        if let Some(screen) = &mut self.cheevos {
            screen.poll();
        }
        if let Some(result) = self.achievements_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.achievements_rx = None;
            match result {
                Ok(a) => self.achievements = a,
                Err(e) => self.set_status(Level::Warn, format!("RetroAchievements: {e}")),
            }
        }
        if let Some(found) = self.metadata.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.metadata = None;
            crate::meta::apply(&mut self.lib, found);
            self.filter_options = filter::Options::from(&self.lib, &self.state);
            self.rebuild_keeping_selection();
        }
        let Some(run) = &mut self.running else { return };
        let status = match run.child.try_wait() {
            Ok(None) => return,
            Ok(Some(s)) => Some(s),
            Err(_) => None,
        };
        let run = self.running.take().unwrap();
        let (played, state, _) = run.session.end(status);
        self.state = state;
        self.finish(run.game, played, status.map(|s| s.code()));
    }

    /// Back to the list after a game exits (wait or terminal mode). Its session has already
    /// been recorded.
    pub fn finish(&mut self, g: usize, played: Duration, code: Option<Option<i32>>) {
        let name = self.lib.games[g].name.clone();
        // Whatever window had focus is gone; assume we're back in front.
        self.term_focused = true;
        self.cooldown_until = Some(Instant::now() + INPUT_COOLDOWN);
        match code {
            Some(Some(0)) => {
                self.set_status(Level::Info, format!("Played {name} for {}", crate::text::duration(played.as_secs())))
            }
            Some(Some(c)) => self
                .set_status(Level::Error, format!("{name} exited with code {c} (log: {})", config::tilde(&log_path()))),
            Some(None) => self.set_status(Level::Warn, format!("{name} was killed by a signal")),
            None => self.set_status(Level::Error, format!("Lost track of {name}")),
        }
        self.rebuild_keeping_selection();
    }

    pub fn reload(&mut self) {
        self.reload_as("Reloaded");
    }

    /// Reads the config again and rebuilds everything that comes from it, saying `done` and how
    /// many games there are.
    fn reload_as(&mut self, done: &str) {
        match config::load(&self.cfg_path) {
            Ok(cfg) => {
                self.lib = Library::load(&cfg);
                self.emulators = Emulators::default();
                if let Some(c) = &mut self.covers {
                    c.reset(&cfg);
                }
                self.cfg = cfg;
                self.apply_settings();
                self.load_metadata();
                self.load_achievements();
                self.filter_options = filter::Options::from(&self.lib, &self.state);
                self.end_search();
                self.rebuild_keeping_selection();
                self.set_status(Level::Info, format!("{done}: {} games", self.lib.games.len()));
            }
            Err(e) => self.set_status(Level::Error, format!("Config error: {e:#}")),
        }
        // Even after an error, so it isn't reported again until the files change again.
        self.watched = Watched::new(&self.cfg_path, &self.cfg);
    }

    /// Puts the config's settings into effect where they're kept outside it.
    pub fn apply_settings(&mut self) {
        use crate::emu::wireframe::{Look, parse_colors};
        let listed: Vec<[u8; 3]> =
            self.cfg.launch_animation.colors.iter().filter_map(|c| parse_colors([c.as_str()]).ok()).flatten().collect();
        self.look = Look::new((!listed.is_empty()).then_some(listed), self.terminal_colors.clone());
        self.swap_confirm.store(self.cfg.controller.swap_confirm, Ordering::Relaxed);
    }

    pub fn save_selection(&mut self) {
        let system = self.views.get(self.view).map(|v| v.id.clone());
        let game = self.selected_game().map(|g| self.lib.games[g].key.clone());
        self.state = State::update(|s| {
            s.selected_system = system;
            s.selected_game = game;
        });
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
