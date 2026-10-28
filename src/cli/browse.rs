//! `romi list` and `romi info`: the library as text or JSON.

use super::*;

impl Filters {
    pub(super) fn none() -> Filters {
        Filters {
            system: Vec::new(),
            genre: None,
            players: None,
            decade: None,
            favorites: false,
            played: false,
            unplayed: false,
            tag: None,
            hidden: false,
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
        let tag = match &self.tag {
            Some(t) => Some(find_tag(state, t)?),
            None => None,
        };
        let filter = Filter { genre, players: self.players, decade: self.decade, played, tag };
        Ok(systems
            .iter()
            .flat_map(|&s| lib.systems[s].games.iter().copied())
            .filter(|&g| filter.matches(&lib.games[g], state))
            .filter(|&g| !self.favorites || state.is_favorite(&lib.games[g].key))
            .filter(|&g| self.hidden || !lib.games[g].hidden)
            .collect())
    }
}

pub(super) fn list(cfg: &Config, filters: &Filters, json: bool) -> Result<()> {
    let lib = Library::load_complete(cfg);
    let state = State::load();
    let games = filters.apply(&lib, &state)?;
    let mut out = stdout().lock();
    if json {
        let mut finder = art::Finder::new(cfg);
        let ach = crate::ra::Achievements::cached(cfg, &lib);
        let records: Vec<_> = games.iter().map(|&g| GameJson::new(&lib, g, &state, &ach, &mut finder)).collect();
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
    /// The ROM file (the first disc of a multi-disc game); null for scripts, ports and Steam games.
    path: Option<String>,
    /// Every disc of a multi-disc game, in order; empty otherwise.
    discs: Vec<String>,
    favorite: bool,
    /// Hidden from lists in romi's menus; `list` leaves these out without `--hidden`.
    hidden: bool,
    /// The emulator picked in the game's menu; null when it runs the system's default.
    emulator: Option<&'a str>,
    /// Your tags for it, in name order.
    tags: Vec<&'a str>,
    /// What RetroAchievements has for it, from the last refresh; null if it has nothing or
    /// RetroAchievements isn't set up.
    achievements: Option<&'a crate::ra::Info>,
    playtime_secs: u64,
    /// Unix time.
    last_played: Option<u64>,
    /// Box art found on disk.
    art: Option<String>,
    /// The title screen found on disk.
    title_screen: Option<String>,
    /// A screenshot found on disk.
    screenshot: Option<String>,
    meta: &'a Meta,
}

impl<'a> GameJson<'a> {
    fn new(
        lib: &'a Library,
        g: usize,
        state: &'a State,
        ach: &'a crate::ra::Achievements,
        finder: &mut art::Finder,
    ) -> GameJson<'a> {
        let game = &lib.games[g];
        let system = lib.system_of(g);
        GameJson {
            key: &game.key,
            name: &game.name,
            system: &system.id,
            system_name: &system.name,
            path: game.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            discs: game.discs.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
            favorite: state.is_favorite(&game.key),
            hidden: game.hidden,
            emulator: game.emulator.as_deref(),
            tags: state.tags_of(&game.key),
            achievements: ach.games.get(&game.key),
            playtime_secs: state.playtime.get(&game.key).copied().unwrap_or(0),
            last_played: state.last_played.get(&game.key).copied(),
            art: finder.find(system, game).map(|(p, _)| p.to_string_lossy().into_owned()),
            title_screen: finder.find_kind(system, game, Kind::Title).map(|(p, _)| p.to_string_lossy().into_owned()),
            screenshot: finder.find_kind(system, game, Kind::Snap).map(|(p, _)| p.to_string_lossy().into_owned()),
            meta: &game.meta,
        }
    }
}

pub(super) fn info(cfg: &Config, query: &str, system: Option<&str>, json: bool) -> Result<()> {
    let lib = Library::load_complete(cfg);
    let g = resolve(&lib, query, system)?;
    let state = State::load();
    let mut finder = art::Finder::new(cfg);
    let ach = crate::ra::Achievements::cached(cfg, &lib);
    let record = GameJson::new(&lib, g, &state, &ach, &mut finder);
    if json {
        println!("{}", serde_json::to_string_pretty(&record)?);
        return Ok(());
    }
    let game = &lib.games[g];
    let m = &game.meta;
    let tilde = |p: &str| config::tilde(std::path::Path::new(p));
    let mut emulators = crate::emulators::Emulators::default();
    emulators.find(cfg, &lib, g);
    let options = emulators.options(&lib, g);
    let played = (record.playtime_secs > 0 || record.last_played.is_some()).then(|| {
        let mut s = crate::text::duration(record.playtime_secs);
        if let Some(at) = record.last_played {
            s.push_str(&format!(", last {}", crate::text::ago(now().saturating_sub(at))));
        }
        s
    });
    println!("{}", game.name);
    for (label, value) in [
        ("key", Some(game.key.clone())),
        ("system", Some(format!("{} ({})", record.system_name, record.system))),
        ("file", record.discs.is_empty().then(|| record.path.as_deref().map(tilde)).flatten()),
        (
            "discs",
            (!record.discs.is_empty())
                .then(|| record.discs.iter().map(|d| tilde(d)).collect::<Vec<_>>().join("\n             ")),
        ),
        ("genre", (!m.genres.is_empty()).then(|| m.genres.join(", "))),
        ("developer", m.developer.clone()),
        ("publisher", m.publisher.clone()),
        ("released", m.year.map(|y| y.to_string())),
        ("players", m.players.map(|n| n.to_string())),
        ("series", m.franchise.clone()),
        ("favorite", record.favorite.then(|| "yes".to_string())),
        ("hidden", record.hidden.then(|| "yes".to_string())),
        ("tags", (!record.tags.is_empty()).then(|| record.tags.join(", "))),
        ("cheevos", record.achievements.map(|a| a.summary())),
        ("played", played),
        ("emulator", options.get(crate::emulators::current(game, &options)).map(|e| e.name.clone())),
        (
            "also",
            (options.len() > 1).then(|| {
                let current = crate::emulators::current(game, &options);
                let others: Vec<&str> =
                    options.iter().enumerate().filter(|&(i, _)| i != current).map(|(_, e)| e.name.as_str()).collect();
                others.join(", ")
            }),
        ),
        ("command", Some(game.launch.command.clone())),
        ("art", record.art.as_deref().map(tilde)),
        ("title", record.title_screen.as_deref().map(tilde)),
        ("screenshot", record.screenshot.as_deref().map(tilde)),
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
