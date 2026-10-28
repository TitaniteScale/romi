//! The command line: its arguments, completion, exit codes, and handing each command to the file
//! for it: `browse` (list, info), `play` (play, run), `setup` (init, add, check) and `reports`
//! (status, stats, achievements).

mod browse;
mod play;
mod reports;
mod setup;

pub use setup::first_run;

use crate::art::Kind;
use crate::config::{self, Config, LaunchMode};
use crate::filter::Filter;
use crate::hooks::Hooks;
use crate::launch::Prepared;
use crate::library::{Game, Library, Resolve};
use crate::meta::Meta;
use crate::session::{self, Session};
use crate::state::{State, now};
use crate::text;
use crate::tui::app;
use crate::{art, steam};
use anyhow::{Context, Result, bail};
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
    /// Run a game inside romi for a number of frames, with no window, and save the last frame.
    /// Uses the libretro core from the game's RetroArch command
    Run {
        /// A ROM file, or a game key (from `romi list`), exact name or words from the name
        #[arg(add = ArgValueCompleter::new(complete_games))]
        game: String,
        /// Only look in this system (id or name)
        #[arg(long, value_name = "ID", add = ArgValueCompleter::new(complete_systems))]
        system: Option<String>,
        /// The libretro core to use instead of the one in the game's command
        #[arg(long, value_name = "PATH")]
        core: Option<PathBuf>,
        /// How many frames to run
        #[arg(long, default_value_t = 600, value_name = "N")]
        frames: u64,
        /// Save the last frame here as a PNG
        #[arg(long, value_name = "PATH")]
        screenshot: Option<PathBuf>,
        /// Save the sound of the whole run here as a WAV file
        #[arg(long, value_name = "PATH")]
        wav: Option<PathBuf>,
        /// Buttons to press: lines of `<frame> <buttons...>`, each held until the next line
        /// (buttons: a b x y l r l2 r2 l3 r3 start select up down left right)
        #[arg(long, value_name = "PATH")]
        input: Option<PathBuf>,
        /// Print the outcome as JSON
        #[arg(long)]
        json: bool,
        /// Play it in this terminal instead, drawn with Kitty graphics (an experiment: Esc quits,
        /// F1 and F2 switch how frames are sent and scaled, F4 mutes)
        #[arg(long, conflicts_with_all = ["frames", "screenshot", "wav", "input", "json"])]
        live: bool,
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
    /// Add a program, script, folder or .desktop file as a game
    Add {
        /// What to add. A folder adds the one program in it
        path: PathBuf,
        /// What to call it (guessed from the file or folder name otherwise)
        #[arg(long)]
        name: Option<String>,
        /// Where to list it: a system id, or any label for a group of its own [default: PC]
        #[arg(long, value_name = "LABEL")]
        system: Option<String>,
        /// How to run it: wait, terminal (the default for scripts), or detach
        #[arg(long, value_enum)]
        mode: Option<LaunchMode>,
        /// Print the entry instead of adding it
        #[arg(long)]
        print: bool,
        /// Add it without asking
        #[arg(long, short)]
        yes: bool,
    },
    /// Show the games running right now, started from the TUI or `romi play`
    Status {
        /// Print a JSON array instead
        #[arg(long)]
        json: bool,
    },
    /// Run a command whenever a controller's Guide button is pressed, outside of games: for
    /// opening romi from the couch. Runs until stopped; start it with your desktop
    Guide {
        /// The command and its arguments, e.g. `romi guide -- hyprctl eval 'romi_couch()'`
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true, value_name = "COMMAND")]
        command: Vec<String>,
    },
    /// Refresh your RetroAchievements progress and list your games that have achievements
    Achievements {
        /// Print JSON instead
        #[arg(long)]
        json: bool,
    },
    /// Show play time per system, the most-played games, and this year month by month
    Stats {
        /// Print JSON instead
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
    /// Only games with this tag of yours
    #[arg(long, add = ArgValueCompleter::new(complete_tags))]
    tag: Option<String>,
    /// Include games hidden from romi's menus
    #[arg(long)]
    hidden: bool,
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
        .map(|(g, n)| (g, format!("{n} {}", text::plural(n, "game"))))
        .collect();
    candidates(current, genres.iter().map(|(g, n)| (g.as_str(), n.as_str())))
}

fn complete_tags(current: &OsStr) -> Vec<CompletionCandidate> {
    let state = State::load();
    let tags: Vec<(String, String)> = state
        .tags
        .iter()
        .map(|(t, keys)| (t.clone(), format!("{} {}", keys.len(), text::plural(keys.len(), "game"))))
        .collect();
    candidates(current, tags.iter().map(|(t, n)| (t.as_str(), n.as_str())))
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

/// Loads the config. There's no default to fall back on: one that points at the wrong ROM folder
/// only gets in the way of `romi init` finding the right one.
pub fn load_config(path: &Path) -> Result<Config> {
    if !path.exists() {
        return fail(exit::CONFIG, format!("no config at {} yet; run `romi init` first", config::tilde(path)));
    }
    config::load(path).or_else(|e| fail(exit::CONFIG, format!("{e:#}")))
}

pub fn run(command: Command, cfg_path: &Path) -> Result<()> {
    // init works without a config (that's what it's for).
    match command {
        Command::Init { roms, print, yes, esde, no_esde } => {
            return setup::init(cfg_path, roms, print, yes, esde, no_esde, false);
        }
        // add edits the config's text, so it reads the config itself.
        Command::Add { path, name, system, mode, print, yes } => {
            return setup::add(cfg_path, &path, name, system, mode, print, yes);
        }
        // guide only watches the controller.
        Command::Guide { command } => return crate::gamepad::run_on_guide(&command),
        // status only reads the state folder.
        Command::Status { json } => return reports::status(json),
        _ => {}
    }
    let cfg = &load_config(cfg_path)?;
    match command {
        Command::Init { .. } | Command::Add { .. } | Command::Guide { .. } | Command::Status { .. } => unreachable!(),
        Command::Check => setup::check(cfg, cfg_path),
        Command::List { filters, json } => browse::list(cfg, &filters, json),
        Command::Info { game, system, json } => browse::info(cfg, &game, system.as_deref(), json),
        Command::Play { game, system, mode, dry_run, json } => {
            play::play(cfg, cfg_path, &game, system.as_deref(), mode, dry_run, json)
        }
        Command::Run { game, system, core, frames, screenshot, wav, input, json, live } => {
            let (screenshot, wav) = (screenshot.as_deref(), wav.as_deref());
            play::run_core(cfg, &game, system.as_deref(), core, frames, screenshot, wav, input.as_deref(), json, live)
        }
        Command::Stats { json } => reports::stats(cfg, json),
        Command::Achievements { json } => reports::achievements(cfg, json),
        Command::Scrape { systems } => art::scrape(cfg, &Library::load_complete(cfg), &systems),
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

fn find_tag(state: &State, wanted: &str) -> Result<String> {
    let tag = state.tag_named(wanted);
    if state.tags.contains_key(&tag) {
        return Ok(tag);
    }
    let names: Vec<&str> = state.tags.keys().map(String::as_str).collect();
    let known = if names.is_empty() { "you have none yet".to_string() } else { format!("tags: {}", names.join(", ")) };
    fail(exit::USAGE, format!("no tag {wanted:?}; {known}"))
}

fn find_genre(lib: &Library, wanted: &str) -> Result<String> {
    let genres = crate::meta::genre_counts(lib);
    if let Some((g, _)) = genres.iter().find(|(g, _)| g.eq_ignore_ascii_case(wanted)) {
        return Ok(g.clone());
    }
    let names: Vec<&str> = genres.iter().map(|(g, _)| g.as_str()).collect();
    fail(exit::USAGE, format!("no games with genre {wanted:?}; genres: {}", names.join(", ")))
}

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
