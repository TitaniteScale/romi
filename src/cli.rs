//! The command line: `romi check`, `list`, `info` and `scrape`, plus the flags they replaced.

use crate::config::{self, Config, LaunchMode};
use crate::filter::Filter;
use crate::launch::Prepared;
use crate::library::{Game, Library, Resolve};
use crate::meta::Meta;
use crate::session::{self, Session};
use crate::state::{State, now};
use crate::{app, art, steam};
use anyhow::Result;
use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate};
use serde::Serialize;
use std::ffi::OsStr;
use std::io::{Write, stdout};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

/// Shown under `romi --help`.
const KEYS: &str = "\
keys:
  j/k ↑/↓   move           h/l ←/→   switch pane      tab/[ ]   next/prev system
  enter     play           /         search           f         favorite
  r         random         d         details pane     m         menu
  F         filter         ctrl-r    reload config    q         quit

controller:
  d-pad/stick move (hold to speed up)   A play   B back   Y favorite   X random
  LB/RB system   LT/RT previous/next letter   Select search   Start menu (incl. quit)
  R3 (click right stick) filter by genre, players, decade, played
  searching: d-pad picks a key, A types, X deletes, Y space, B done
";

/// romi's own exit codes. They come from the sysexits range, which games rarely use, so a script
/// can tell romi's failures apart from a game's exit code.
pub mod exit {
    /// Bad arguments (EX_USAGE).
    pub const USAGE: i32 = 64;
    /// A game name matched more than one game (EX_DATAERR).
    pub const AMBIGUOUS: i32 = 65;
    /// No game matched (EX_NOINPUT).
    pub const NOT_FOUND: i32 = 66;
    /// The game's emulator or program isn't installed (EX_UNAVAILABLE).
    pub const UNAVAILABLE: i32 = 69;
    /// The config can't be read, or a game's command in it is malformed (EX_CONFIG).
    pub const CONFIG: i32 = 78;
}

/// An error that ends romi with a particular exit code.
#[derive(Debug)]
pub struct Exit {
    pub code: i32,
    pub message: String,
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Exit {}

fn fail<T>(code: i32, message: impl Into<String>) -> Result<T> {
    Err(Exit { code, message: message.into() }.into())
}

/// romi - a terminal game launcher. Run with no command to open the launcher.
#[derive(Parser)]
#[command(version, after_help = KEYS)]
struct Cli {
    /// Use a different config file (or set ROMI_CONFIG)
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,

    // The flags romi had before subcommands, kept so existing scripts still work.
    #[arg(long, hide = true, conflicts_with_all = ["list", "scrape"])]
    check: bool,
    #[arg(long, hide = true, conflicts_with = "scrape")]
    list: bool,
    #[arg(long, hide = true, num_args = 0.., value_name = "ID")]
    scrape: Option<Vec<String>>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Show what romi finds and anything that looks broken
    Check,
    /// List games as tab-separated system, name, key (or JSON)
    List {
        #[command(flatten)]
        filters: Filters,
        /// Print a JSON array of games instead
        #[arg(long)]
        json: bool,
    },
    /// Show everything romi knows about one game
    Info {
        /// A game key (from `romi list`), an exact name, or words from the name
        #[arg(add = ArgValueCompleter::new(complete_games))]
        game: String,
        /// Only look in this system (id or name)
        #[arg(long, value_name = "ID", add = ArgValueCompleter::new(complete_systems))]
        system: Option<String>,
        /// Print JSON instead
        #[arg(long)]
        json: bool,
    },
    /// Start a game. Exits with the game's own exit code
    Play {
        /// A game key (from `romi list`), an exact name, or words from the name
        #[arg(add = ArgValueCompleter::new(complete_games))]
        game: String,
        /// Only look in this system (id or name)
        #[arg(long, value_name = "ID", add = ArgValueCompleter::new(complete_systems))]
        system: Option<String>,
        /// Run it this way instead of how the config says: wait (output goes to a log), terminal
        /// (output comes here), or detach (start it and return)
        #[arg(long, value_enum)]
        mode: Option<LaunchMode>,
        /// Print the command, working directory and environment instead of running it
        #[arg(long)]
        dry_run: bool,
        /// Print the outcome (or with --dry-run, the command) as JSON
        #[arg(long)]
        json: bool,
    },
    /// Find your ROM folders and emulators, and write a config for them
    Init {
        /// Where your ROM folders are (found automatically if not given)
        #[arg(long, value_name = "DIR")]
        roms: Option<PathBuf>,
        /// Print the generated config instead of writing anything
        #[arg(long)]
        print: bool,
        /// Write without asking
        #[arg(long, short)]
        yes: bool,
        /// Folder with ES-DE's es_systems.xml and es_find_rules.xml (found automatically when
        /// ES-DE is installed)
        #[arg(long, value_name = "DIR")]
        esde: Option<PathBuf>,
        /// Use romi's built-in system list even if ES-DE is installed
        #[arg(long, conflicts_with = "esde")]
        no_esde: bool,
    },
    /// Show the games running right now, started from the TUI or `romi play`
    Status {
        /// Print a JSON array instead
        #[arg(long)]
        json: bool,
    },
    /// Find box art on disk and download the rest
    Scrape {
        /// Only these systems (ids from the config); all of them if none are given
        #[arg(value_name = "ID", add = ArgValueCompleter::new(complete_systems))]
        systems: Vec<String>,
    },
}

/// The same filters as the TUI's, plus systems and favorites.
#[derive(Args)]
pub struct Filters {
    /// Only this system (id or name); repeat for more
    #[arg(long, value_name = "ID", add = ArgValueCompleter::new(complete_systems))]
    system: Vec<String>,
    /// Only this genre (an unknown one lists them all)
    #[arg(long, add = ArgValueCompleter::new(complete_genres))]
    genre: Option<String>,
    /// At least this many players
    #[arg(long, value_name = "N")]
    players: Option<u8>,
    /// Released in this decade: 1990, 1990s or 90s
    #[arg(long, value_parser = parse_decade)]
    decade: Option<u16>,
    /// Only favorites
    #[arg(long)]
    favorites: bool,
    /// Only games played from romi before
    #[arg(long, conflicts_with = "unplayed")]
    played: bool,
    /// Only games never played from romi
    #[arg(long)]
    unplayed: bool,
}

fn parse_decade(s: &str) -> Result<u16, String> {
    let digits = s.strip_suffix('s').unwrap_or(s);
    let n: u16 = digits.parse().map_err(|_| format!("not a decade: {s}"))?;
    let year = match (digits.len(), n) {
        (2, 30..) => 1900 + n,
        (2, _) => 2000 + n,
        (4, _) => n,
        _ => return Err(format!("not a decade: {s}")),
    };
    if year % 10 != 0 {
        return Err(format!("not a decade: {s} (try {}s)", year / 10 * 10));
    }
    Ok(year)
}

/// Answers the shell when it asks for completions (it runs romi with `COMPLETE=<shell>` set), then
/// exits. Does nothing on a normal run. Must come before anything is printed.
pub fn complete() {
    clap_complete::CompleteEnv::with_factory(Cli::command).complete();
}

/// The library for completions. Reads the config only if it exists: completing must never write
/// the default one. (`--config` on the command line being completed isn't seen here.)
fn completion_library(with_metadata: bool) -> Option<Library> {
    let path = config::config_path();
    let cfg = path.exists().then(|| config::load(&path).ok()).flatten()?;
    Some(if with_metadata { Library::load_complete(&cfg) } else { Library::load(&cfg) })
}

/// Values that start with what's typed so far, ignoring case.
fn candidates<'a>(current: &OsStr, values: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<CompletionCandidate> {
    let typed = current.to_string_lossy().to_lowercase();
    values
        .filter(|(value, _)| value.to_lowercase().starts_with(&typed))
        .map(|(value, help)| CompletionCandidate::new(value).help(Some(help.to_string().into())))
        .collect()
}

/// Game names, which read better than keys. A name two games share completes to their keys.
fn complete_games(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(lib) = completion_library(false) else { return Vec::new() };
    let mut seen = std::collections::HashMap::new();
    for g in &lib.games {
        *seen.entry(g.name.as_str()).or_insert(0) += 1;
    }
    let values = lib.games.iter().map(|g| (if seen[g.name.as_str()] > 1 { &g.key } else { &g.name }, &g.system));
    candidates(current, values.map(|(v, s)| (v.as_str(), s.as_str())))
}

fn complete_systems(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(lib) = completion_library(false) else { return Vec::new() };
    candidates(current, lib.systems.iter().map(|s| (s.id.as_str(), s.name.as_str())))
}

fn complete_genres(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(lib) = completion_library(true) else { return Vec::new() };
    let genres: Vec<(String, String)> = crate::meta::genre_counts(&lib)
        .into_iter()
        .map(|(g, n)| (g, format!("{n} game{}", if n == 1 { "" } else { "s" })))
        .collect();
    candidates(current, genres.iter().map(|(g, n)| (g.as_str(), n.as_str())))
}

/// Parses the arguments, exiting on bad ones. Returns the command (None opens the TUI) and the
/// config path.
pub fn parse() -> (Option<Command>, PathBuf) {
    let cli = Cli::try_parse().unwrap_or_else(|e| exit_with(e));
    let command = match (cli.command, cli.check, cli.list, cli.scrape) {
        // clap can't declare a conflict between flags and a subcommand without also refusing
        // global options like --config before the subcommand, so this one is checked here.
        (Some(_), true, ..) | (Some(_), _, true, _) | (Some(_), .., Some(_)) => {
            let msg = "--check, --list and --scrape can't be combined with a command";
            exit_with(Cli::command().error(clap::error::ErrorKind::ArgumentConflict, msg))
        }
        (Some(c), ..) => Some(c),
        (None, true, ..) => Some(Command::Check),
        (None, _, true, _) => Some(Command::List { filters: Filters::none(), json: false }),
        (None, _, _, Some(systems)) => Some(Command::Scrape { systems }),
        _ => None,
    };
    (command, cli.config.unwrap_or_else(config::config_path))
}

/// Prints a parse error (or the help or version, which clap reports the same way) and exits.
fn exit_with(e: clap::Error) -> ! {
    let _ = e.print();
    std::process::exit(if e.use_stderr() { exit::USAGE } else { 0 })
}

/// Loads the config, writing the default one if there's none yet.
pub fn load_config(path: &Path) -> Result<Config> {
    config::load(path).or_else(|e| fail(exit::CONFIG, format!("{e:#}")))
}

pub fn run(command: Command, cfg_path: &Path) -> Result<()> {
    // init works without a config (that's what it's for), so it mustn't make the default one.
    if let Command::Init { roms, print, yes, esde, no_esde } = command {
        return init(cfg_path, roms, print, yes, esde, no_esde);
    }
    let cfg = &load_config(cfg_path)?;
    match command {
        Command::Init { .. } => unreachable!(),
        Command::Check => check(cfg, cfg_path),
        Command::List { filters, json } => list(cfg, &filters, json),
        Command::Info { game, system, json } => info(cfg, &game, system.as_deref(), json),
        Command::Play { game, system, mode, dry_run, json } => {
            play(cfg, cfg_path, &game, system.as_deref(), mode, dry_run, json)
        }
        Command::Status { json } => status(json),
        Command::Scrape { systems } => art::scrape(cfg, &Library::load_complete(cfg), &systems),
    }
}

// ---- list ----

impl Filters {
    fn none() -> Filters {
        Filters {
            system: Vec::new(),
            genre: None,
            players: None,
            decade: None,
            favorites: false,
            played: false,
            unplayed: false,
        }
    }

    /// The games that pass, in the TUI's order: by system, then alphabetically.
    fn apply(&self, lib: &Library, state: &State) -> Result<Vec<usize>> {
        let mut systems = self.system.iter().map(|s| find_system(lib, s)).collect::<Result<Vec<_>>>()?;
        if systems.is_empty() {
            systems = (0..lib.systems.len()).collect();
        }
        systems.sort_unstable();
        systems.dedup();
        let genre = match &self.genre {
            Some(g) => Some(find_genre(lib, g)?),
            None => None,
        };
        let played = if self.played { Some(true) } else { self.unplayed.then_some(false) };
        let filter = Filter { genre, players: self.players, decade: self.decade, played };
        Ok(systems
            .iter()
            .flat_map(|&s| lib.systems[s].games.iter().copied())
            .filter(|&g| filter.matches(&lib.games[g], state))
            .filter(|&g| !self.favorites || state.is_favorite(&lib.games[g].key))
            .collect())
    }
}

fn find_system(lib: &Library, wanted: &str) -> Result<usize> {
    if let Some(i) =
        lib.systems.iter().position(|s| s.id.eq_ignore_ascii_case(wanted) || s.name.eq_ignore_ascii_case(wanted))
    {
        return Ok(i);
    }
    let ids: Vec<&str> = lib.systems.iter().map(|s| s.id.as_str()).collect();
    fail(exit::USAGE, format!("no system called {wanted:?}; systems: {}", ids.join(", ")))
}

fn find_genre(lib: &Library, wanted: &str) -> Result<String> {
    let genres = crate::meta::genre_counts(lib);
    if let Some((g, _)) = genres.iter().find(|(g, _)| g.eq_ignore_ascii_case(wanted)) {
        return Ok(g.clone());
    }
    let names: Vec<&str> = genres.iter().map(|(g, _)| g.as_str()).collect();
    fail(exit::USAGE, format!("no games with genre {wanted:?}; genres: {}", names.join(", ")))
}

fn list(cfg: &Config, filters: &Filters, json: bool) -> Result<()> {
    let lib = Library::load_complete(cfg);
    let state = State::load();
    let games = filters.apply(&lib, &state)?;
    let mut out = stdout().lock();
    if json {
        let mut finder = art::Finder::new(cfg);
        let records: Vec<_> = games.iter().map(|&g| GameJson::new(&lib, g, &state, &mut finder)).collect();
        serde_json::to_writer_pretty(&mut out, &records)?;
        writeln!(out)?;
    } else {
        for g in games.iter().map(|&g| &lib.games[g]) {
            writeln!(out, "{}\t{}\t{}", g.system, g.name, g.key)?;
        }
    }
    Ok(())
}

/// A game as `list --json` and `info --json` print it. The field names are part of romi's
/// command-line interface: add fields freely, but don't rename or remove them.
#[derive(Serialize)]
struct GameJson<'a> {
    key: &'a str,
    name: &'a str,
    /// System id, as used by `--system`.
    system: &'a str,
    system_name: &'a str,
    /// The ROM file; null for scripts, ports and Steam games.
    path: Option<String>,
    favorite: bool,
    playtime_secs: u64,
    /// Unix time.
    last_played: Option<u64>,
    /// Box art found on disk.
    art: Option<String>,
    meta: &'a Meta,
}

impl<'a> GameJson<'a> {
    fn new(lib: &'a Library, g: usize, state: &State, finder: &mut art::Finder) -> GameJson<'a> {
        let game = &lib.games[g];
        let system = lib.system_of(g);
        GameJson {
            key: &game.key,
            name: &game.name,
            system: &system.id,
            system_name: &system.name,
            path: game.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            favorite: state.is_favorite(&game.key),
            playtime_secs: state.playtime.get(&game.key).copied().unwrap_or(0),
            last_played: state.last_played.get(&game.key).copied(),
            art: finder.find(system, game).map(|(p, _)| p.to_string_lossy().into_owned()),
            meta: &game.meta,
        }
    }
}

// ---- info ----

/// Finds one game, or fails with the exit code for "not found" or "ambiguous".
fn resolve(lib: &Library, query: &str, system: Option<&str>) -> Result<usize> {
    let system = system.map(|s| find_system(lib, s)).transpose()?;
    match lib.resolve(query, system) {
        Ok(g) => Ok(g),
        Err(Resolve::NotFound) => fail(exit::NOT_FOUND, format!("no game matches {query:?} (see `romi list`)")),
        Err(Resolve::Ambiguous(games)) => {
            const SHOWN: usize = 20;
            let mut msg = format!("{query:?} matches {} games; use a key, or narrow it with --system:", games.len());
            for g in games.iter().take(SHOWN).map(|&g| &lib.games[g]) {
                msg.push_str(&format!("\n  {}  ({}: {})", g.key, g.system, g.name));
            }
            if games.len() > SHOWN {
                msg.push_str(&format!("\n  ...and {} more", games.len() - SHOWN));
            }
            fail(exit::AMBIGUOUS, msg)
        }
    }
}

fn info(cfg: &Config, query: &str, system: Option<&str>, json: bool) -> Result<()> {
    let lib = Library::load_complete(cfg);
    let g = resolve(&lib, query, system)?;
    let state = State::load();
    let mut finder = art::Finder::new(cfg);
    let record = GameJson::new(&lib, g, &state, &mut finder);
    if json {
        println!("{}", serde_json::to_string_pretty(&record)?);
        return Ok(());
    }
    let game = &lib.games[g];
    let m = &game.meta;
    let tilde = |p: &str| app::tilde(std::path::Path::new(p));
    let played = (record.playtime_secs > 0 || record.last_played.is_some()).then(|| {
        let mut s = crate::ui::fmt_duration(record.playtime_secs);
        if let Some(at) = record.last_played {
            s.push_str(&format!(", last {}", crate::ui::fmt_ago(now().saturating_sub(at))));
        }
        s
    });
    println!("{}", game.name);
    for (label, value) in [
        ("key", Some(game.key.clone())),
        ("system", Some(format!("{} ({})", record.system_name, record.system))),
        ("file", record.path.as_deref().map(tilde)),
        ("genre", (!m.genres.is_empty()).then(|| m.genres.join(", "))),
        ("developer", m.developer.clone()),
        ("publisher", m.publisher.clone()),
        ("released", m.year.map(|y| y.to_string())),
        ("players", m.players.map(|n| n.to_string())),
        ("series", m.franchise.clone()),
        ("favorite", record.favorite.then(|| "yes".to_string())),
        ("played", played),
        ("command", Some(game.launch.command.clone())),
        ("art", record.art.as_deref().map(tilde)),
    ] {
        if let Some(value) = value {
            println!("  {label:<10} {value}");
        }
    }
    if let Some(desc) = &m.description {
        println!("\n{desc}");
    }
    Ok(())
}

// ---- play ----

fn play(
    cfg: &Config,
    cfg_path: &std::path::Path,
    query: &str,
    system: Option<&str>,
    mode: Option<LaunchMode>,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    // No metadata needed, so this skips the database lookups `list` and `info` do.
    let lib = Library::load(cfg);
    let g = resolve(&lib, query, system)?;
    let game = &lib.games[g];
    let mode = mode.unwrap_or(game.launch.mode);
    let prepared = match game.launch.prepare() {
        Ok(p) => p,
        Err(e) => return fail(exit::CONFIG, format!("{}: {e:#}", game.name)),
    };
    if dry_run {
        return print_command(game, &prepared, mode, json);
    }
    if let Err(e) = prepared.check_program() {
        return fail(exit::UNAVAILABLE, format!("{}: {e:#}", game.name));
    }
    let log = app::log_path();
    if mode == LaunchMode::Detach {
        // A copy of romi in wait mode plays it in the background and records the session.
        session::play_in_background(cfg_path, &game.key)?;
        if json {
            println!("{}", serde_json::json!({ "key": game.key, "name": game.name, "mode": mode }));
        }
        return Ok(());
    }
    catch_stop_signals();
    let mut child = match mode {
        LaunchMode::Terminal => prepared.spawn_attached()?,
        _ => prepared.spawn_logged(&log, false)?,
    };
    let (session, _) = Session::begin(&lib, g, child.id());
    let status = wait_for_game(&mut child)?;
    let (played, _) = session.end();
    // Like a shell: 128 plus the signal number when the game was killed.
    let code = status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
    let log = (mode == LaunchMode::Wait).then(|| log.to_string_lossy().into_owned());
    if json {
        let outcome = serde_json::json!({
            "key": game.key,
            "name": game.name,
            "mode": mode,
            "exit_code": code,
            "played_secs": played.as_secs(),
            "log": log,
        });
        println!("{outcome}");
    }
    if code != 0 {
        let how = match status.signal() {
            Some(sig) => format!("was killed by signal {sig}"),
            None => format!("exited with code {code}"),
        };
        let log = log.map(|l| format!(" (log: {})", app::tilde(std::path::Path::new(&l)))).unwrap_or_default();
        return fail(code, format!("{} {how}{log}", game.name));
    }
    Ok(())
}

fn print_command(game: &Game, prepared: &Prepared, mode: LaunchMode, json: bool) -> Result<()> {
    let cwd = prepared.cwd.as_ref().map(|c| c.to_string_lossy().into_owned());
    if json {
        let env: serde_json::Map<String, serde_json::Value> =
            prepared.env.iter().map(|(k, v)| (k.clone(), v.clone().into())).collect();
        let command = serde_json::json!({
            "key": game.key,
            "name": game.name,
            "mode": mode,
            "argv": prepared.argv,
            "cwd": cwd,
            "env": env,
        });
        println!("{}", serde_json::to_string_pretty(&command)?);
        return Ok(());
    }
    println!("mode     {}", serde_json::to_value(mode)?.as_str().unwrap_or_default());
    println!("command  {}", shell_words::join(&prepared.argv));
    if let Some(cwd) = cwd {
        println!("cwd      {cwd}");
    }
    for (k, v) in &prepared.env {
        println!("env      {k}={}", shell_words::quote(v));
    }
    Ok(())
}

/// The signal that asked romi to stop while a game was running, or 0.
static STOP_SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_stop_signal(sig: libc::c_int) {
    STOP_SIGNAL.store(sig, Ordering::Relaxed);
}

/// Keeps romi alive long enough to record the session when it's told to stop. Must run before
/// the game starts, so there's no moment when a signal would kill romi and leave the game going.
fn catch_stop_signals() {
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: the handler only stores to an atomic. A handler, unlike SIG_IGN, is reset to the
        // default when the game's program is exec'd, so the game still reacts to signals normally.
        unsafe { libc::signal(sig, on_stop_signal as extern "C" fn(libc::c_int) as libc::sighandler_t) };
    }
}

/// Waits for the game to exit. ^C in the terminal, or `timeout` in a script, signals the whole
/// process group, which the game shares with romi, so the game gets it too and romi just waits.
/// A signal sent to romi alone is passed on if the game is still running a second later.
fn wait_for_game(child: &mut Child) -> std::io::Result<ExitStatus> {
    const PASS_ON_AFTER: Duration = Duration::from_secs(1);
    let mut signalled: Option<(i32, Instant)> = None;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        let sig = STOP_SIGNAL.swap(0, Ordering::Relaxed);
        if sig != 0 {
            signalled = Some((sig, Instant::now()));
        }
        if let Some((sig, at)) = signalled
            && at.elapsed() >= PASS_ON_AFTER
        {
            // SAFETY: kill(2) on our own child's pid; it can't be reused before we reap it.
            unsafe { libc::kill(child.id() as libc::pid_t, sig) };
            signalled = None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---- init ----

fn init(
    cfg_path: &Path,
    roms: Option<PathBuf>,
    print: bool,
    yes: bool,
    esde: Option<PathBuf>,
    no_esde: bool,
) -> Result<()> {
    let existing = crate::init::existing_config(cfg_path);
    if cfg_path.exists() && existing.is_none() {
        return fail(exit::CONFIG, format!("{} can't be read; fix it first", app::tilde(cfg_path)));
    }
    let catalog = if no_esde {
        crate::init::Catalog::builtin()
    } else {
        match (crate::init::Catalog::esde(esde.as_deref()), &esde) {
            (Some(c), _) => c,
            (None, Some(dir)) => return fail(exit::USAGE, format!("no ES-DE system list in {}", dir.display())),
            (None, None) => crate::init::Catalog::builtin(),
        }
    };
    let existing_cfg = existing.as_ref().map(|(_, cfg)| cfg);
    let Some(roms_dir) = crate::init::find_roms_dir(roms.as_deref(), existing_cfg) else {
        return fail(exit::NOT_FOUND, "couldn't find your ROMs; say where with --roms DIR");
    };
    if !roms_dir.is_dir() {
        return fail(exit::NOT_FOUND, format!("{} isn't a folder", roms_dir.display()));
    }
    let report = crate::init::scan(&catalog, &roms_dir, existing_cfg);
    let generated = crate::init::render(&report);
    if print {
        eprint!("{}", crate::init::summary(&report));
        print!("{generated}");
        return Ok(());
    }
    print!("{}", crate::init::summary(&report));
    let already_included =
        existing.as_ref().is_some_and(|(_, cfg)| cfg.include.iter().any(|i| i == crate::init::INCLUDE_FILE));
    if existing.is_some()
        && !already_included
        && !report.findings.iter().any(|f| matches!(f, crate::init::Finding::New { .. }))
    {
        println!("\nNothing new to add, so your config is unchanged.");
        return Ok(());
    }

    let include_path = cfg_path.parent().unwrap_or(Path::new(".")).join(crate::init::INCLUDE_FILE);
    let (main, plan) = match &existing {
        Some((raw, cfg)) => match crate::init::add_include(raw, cfg) {
            Some(text) if text == *raw => (None, format!("write {}", app::tilde(&include_path))),
            Some(text) => {
                let plan = format!("write {} and include it from {}", app::tilde(&include_path), app::tilde(cfg_path));
                (Some(text), plan)
            }
            None => {
                let msg = format!(
                    "{} already includes other files; add \"{}\" to its \"include\" list, then run init again",
                    app::tilde(cfg_path),
                    crate::init::INCLUDE_FILE
                );
                return fail(exit::CONFIG, msg);
            }
        },
        None => {
            let plan = format!("write {} and a new {}", app::tilde(&include_path), app::tilde(cfg_path));
            (Some(crate::init::fresh_config(&roms_dir)), plan)
        }
    };
    if !yes {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            return fail(exit::USAGE, "pass --yes to write without asking, or --print to see the config");
        }
        print!("\nThis will {plan}. Go ahead? [Y/n] ");
        stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim().to_lowercase().starts_with('n') {
            println!("Nothing written.");
            return Ok(());
        }
    }
    if let Some(dir) = cfg_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&include_path, generated)?;
    if let Some(text) = main {
        std::fs::write(cfg_path, text)?;
    }
    // Catch anything that would stop romi from starting now, rather than the next time it does.
    let cfg = load_config(cfg_path)?;
    let lib = Library::load(&cfg);
    println!(
        "\nDone: {} games in {} systems. Run `romi` to browse them, or `romi check` for details.",
        lib.games.len(),
        lib.systems.len()
    );
    Ok(())
}

// ---- status ----

fn status(json: bool) -> Result<()> {
    let running = session::running();
    if json {
        let games: Vec<_> = running
            .iter()
            .map(|r| {
                serde_json::json!({
                    "key": r.key,
                    "name": r.name,
                    "system": r.system,
                    "system_name": r.system_name,
                    "pid": r.pid,
                    "started": r.started,
                    "elapsed_secs": now().saturating_sub(r.started),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&games)?);
    } else if running.is_empty() {
        println!("nothing running");
    } else {
        for r in &running {
            let elapsed = crate::ui::fmt_duration(now().saturating_sub(r.started));
            println!("{}  ({}, {elapsed})", r.name, r.system_name);
        }
    }
    Ok(())
}

// ---- check ----

fn check(cfg: &Config, cfg_path: &std::path::Path) -> Result<()> {
    let lib = Library::load_complete(cfg);
    println!("config  {}", app::tilde(cfg_path));
    println!("roms    {}\n", cfg.roms_dir);
    let roms = config::expand_path(&cfg.roms_dir);
    let width = lib.systems.iter().map(|s| s.name.chars().count()).max().unwrap_or(0).max(24);
    let games = |n: usize| format!("{n:>4} game{}", if n == 1 { " " } else { "s" });
    for sys in &cfg.systems {
        let dir = sys.dir.as_deref().unwrap_or(&sys.id);
        let (mark, detail) = match lib.systems.iter().find(|s| s.id == sys.id) {
            Some(s) => match &s.problem {
                Some(p) => ("!", format!("{}  {p}", games(s.games.len()))),
                None => ("✓", games(s.games.len())),
            },
            None if !roms.join(dir).is_dir() && !config::expand_path(dir).is_dir() => ("-", "no folder".into()),
            None => ("-", games(0)),
        };
        println!("  {mark} {:<width$} {}", sys.name, detail.trim_end());
    }
    for s in &lib.systems {
        if !cfg.systems.iter().any(|c| c.id == s.id) {
            println!("  ✓ {:<width$} {}", s.name, games(s.games.len()).trim_end());
        }
    }
    if cfg.steam.enabled && steam::find_steam_dir(cfg.steam.dir.as_deref()).is_none() {
        println!("\n  steam is enabled but no Steam install was found");
    }
    let count = |f: &dyn Fn(&Game) -> bool| lib.games.iter().filter(|g| f(g)).count();
    println!(
        "\nmetadata  {} genre · {} year · {} players · {} description",
        count(&|g| !g.meta.genres.is_empty()),
        count(&|g| g.meta.year.is_some()),
        count(&|g| g.meta.players.is_some()),
        count(&|g| g.meta.description.is_some()),
    );
    println!("\n{} games total", lib.games.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn decades() {
        assert_eq!(parse_decade("1990"), Ok(1990));
        assert_eq!(parse_decade("1990s"), Ok(1990));
        assert_eq!(parse_decade("90s"), Ok(1990));
        assert_eq!(parse_decade("00s"), Ok(2000));
        assert_eq!(parse_decade("20s"), Ok(2020));
        assert!(parse_decade("1995").is_err());
        assert!(parse_decade("nineties").is_err());
    }
}
