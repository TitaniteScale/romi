//! The command line: `romi check`, `list`, `info` and `scrape`, plus the flags they replaced.

use crate::config::{self, Config};
use crate::filter::Filter;
use crate::library::{Game, Library, Resolve};
use crate::meta::Meta;
use crate::state::{State, now};
use crate::{app, art, steam};
use anyhow::Result;
use clap::{Args, CommandFactory, Parser, Subcommand};
use serde::Serialize;
use std::io::{Write, stdout};
use std::path::PathBuf;

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
        game: String,
        /// Only look in this system (id or name)
        #[arg(long, value_name = "ID")]
        system: Option<String>,
        /// Print JSON instead
        #[arg(long)]
        json: bool,
    },
    /// Find box art on disk and download the rest
    Scrape {
        /// Only these systems (ids from the config); all of them if none are given
        #[arg(value_name = "ID")]
        systems: Vec<String>,
    },
}

/// The same filters as the TUI's, plus systems and favorites.
#[derive(Args)]
pub struct Filters {
    /// Only this system (id or name); repeat for more
    #[arg(long, value_name = "ID")]
    system: Vec<String>,
    /// Only this genre (an unknown one lists them all)
    #[arg(long)]
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

pub fn run(command: Command, cfg: &Config, cfg_path: &std::path::Path) -> Result<()> {
    match command {
        Command::Check => check(cfg, cfg_path),
        Command::List { filters, json } => list(cfg, &filters, json),
        Command::Info { game, system, json } => info(cfg, &game, system.as_deref(), json),
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

// ---- check ----

fn check(cfg: &Config, cfg_path: &std::path::Path) -> Result<()> {
    let lib = Library::load_complete(cfg);
    println!("config  {}", app::tilde(cfg_path));
    println!("roms    {}\n", cfg.roms_dir);
    let roms = config::expand_path(&cfg.roms_dir);
    for sys in &cfg.systems {
        let dir = sys.dir.as_deref().unwrap_or(&sys.id);
        let (mark, detail) = match lib.systems.iter().find(|s| s.id == sys.id) {
            Some(s) => match &s.problem {
                Some(p) => ("!", format!("{:>4} games  {p}", s.games.len())),
                None => ("✓", format!("{:>4} games", s.games.len())),
            },
            None if !roms.join(dir).is_dir() && !config::expand_path(dir).is_dir() => ("-", "no folder".into()),
            None => ("-", "   0 games".into()),
        };
        println!("  {mark} {:<24} {detail}", sys.name);
    }
    for s in &lib.systems {
        if !cfg.systems.iter().any(|c| c.id == s.id) {
            println!("  ✓ {:<24} {:>4} games", s.name, s.games.len());
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
