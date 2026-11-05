//! The TUI's state: the library's views, the selection and what's on screen. What changes it is in
//! the files below this one, starting with `input`.

mod input;
mod login;
mod menu;
mod playing;
mod screens;
mod search;

pub use menu::{ArtPrompt, Menu};

use crate::config::{self, Config};
use crate::emulators::Emulators;
use crate::filter::{self, Filter};
use crate::launch::Prepared;
use crate::library::Library;
use crate::session::Session;
use crate::sort::Sort;
use crate::state::State;
use crate::tui::cover::Covers;
use fuzzy_matcher::skim::SkimMatcherV2;
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
    /// What's recording the game in `embedded`, as an emulator's would be.
    embedded_session: Option<Session>,
    /// The middle panel, where a game played here goes, as it was last drawn.
    pub middle: ratatui::layout::Rect,
    /// The colors the launch animation picks from, and whether the terminal's background is light.
    pub look: crate::emu::wireframe::Look,
    /// What the terminal said its colors are, when romi started.
    pub terminal_colors: crate::emu::wireframe::TerminalColors,
    /// The terminal says where the mouse is in pixels, when asked.
    pub pixel_mouse: bool,
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
    /// The prompt for a game's new cover, while it's open.
    pub art_prompt: Option<ArtPrompt>,
    /// The other emulators for each system, found the first time one of its games' menus opens.
    emulators: Emulators,
    /// Cursor (row, column) while the on-screen keyboard is open.
    pub osk: Option<(usize, usize)>,
    /// The RetroAchievements login screen, while it's open.
    pub login: Option<crate::tui::login::LoginScreen>,
    /// Who's logged in to RetroAchievements, as the options panel shows it.
    pub account: Option<String>,
    /// Letter (or year) shown briefly after jumping to it.
    flash: Option<(String, Instant)>,
    pub filter: Filter,
    pub filter_options: filter::Options,
    /// Selected row while the filter panel is open.
    pub filter_panel: Option<usize>,
    /// The options panel, open on this row.
    pub options: Option<usize>,
    /// The controls screen, while it's open.
    pub controls: Option<crate::tui::controls::Screen>,
    /// The players screen, while it's open.
    pub players: Option<crate::tui::players::Screen>,
    /// The controllers connected, by name (see `emu::controls::pad_names`).
    pub pads: Vec<String>,
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
    /// Whether it's been said why A starts emulators in this terminal.
    told_about_terminal: bool,
    /// What to say when the game that's running exits, since it won't be seen until then.
    after_game: Option<String>,
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
            embedded_session: None,
            middle: ratatui::layout::Rect::default(),
            look: crate::emu::wireframe::Look::new(None, Default::default()),
            terminal_colors: Default::default(),
            pixel_mouse: false,
            swap_confirm,
            watched,
            cores_tried: HashSet::new(),
            pad,
            terminal_job: None,
            covers: None,
            menu: None,
            show_hidden: false,
            tag_name: None,
            art_prompt: None,
            emulators: Emulators::default(),
            osk: None,
            login: None,
            account: None,
            flash: None,
            metadata: None,
            filter: Filter::default(),
            filter_options: filter::Options::default(),
            filter_panel: None,
            options: None,
            controls: None,
            players: None,
            pads: Vec::new(),
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
            told_about_terminal: false,
            after_game: None,
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
        crate::emu::cheevos::resend_waiting();
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

    /// Picks up favorites and play history recorded by other romi processes, like `romi play`.
    fn reload_state(&mut self) {
        let state = State::load();
        if state != self.state {
            self.state = state;
            self.rebuild_keeping_selection();
        }
    }

    /// Whether a game shows in lists and search.
    fn visible(&self, g: usize) -> bool {
        self.show_hidden || !self.lib.games[g].hidden
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
        self.poll_login();
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
