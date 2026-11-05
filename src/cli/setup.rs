//! Setting romi up: `romi init`, `romi add`, the first run, and `romi check`.

use super::*;

/// Asks before writing. False (after saying nothing was written) if the answer is no.
fn confirm(question: &str, yes: bool) -> Result<bool> {
    use std::io::IsTerminal;
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return fail(exit::USAGE, "pass --yes to write without asking, or --print to only show what would be written");
    }
    print!("{question} [Y/n] ");
    stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if answer.trim().to_lowercase().starts_with('n') {
        println!("Nothing written.");
        return Ok(false);
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn add(
    cfg_path: &Path,
    path: &Path,
    name: Option<String>,
    system: Option<String>,
    mode: Option<LaunchMode>,
    print: bool,
    yes: bool,
) -> Result<()> {
    use crate::add::{GAMES_FILE, Refusal};
    let Some((raw, own)) = crate::init::existing_config(cfg_path) else {
        return fail(exit::CONFIG, format!("no config at {} yet; run `romi init` first", config::tilde(cfg_path)));
    };
    let cfg = load_config(cfg_path)?;
    let shown = config::tilde(path);
    let mut entry = match crate::add::entry_for(path, Some(&cfg)) {
        Ok(entry) => entry,
        Err(Refusal::Missing) => return fail(exit::NOT_FOUND, format!("{shown} doesn't exist")),
        Err(Refusal::NothingToRun) => return fail(exit::NOT_FOUND, format!("nothing to run in {shown}")),
        Err(Refusal::Several(programs)) => {
            let msg = format!("{shown} has several programs; add the one to run:\n  {}", programs.join("\n  "));
            return fail(exit::AMBIGUOUS, msg);
        }
        Err(Refusal::NotExecutable) => {
            let msg = format!("{shown} isn't executable (`chmod +x` it first, or add it to config.json by hand)");
            return fail(exit::USAGE, msg);
        }
        Err(Refusal::Rom { system, folder }) => {
            let msg = format!("{shown} looks like a {system} game; put it in {} instead", config::tilde(&folder));
            return fail(exit::USAGE, msg);
        }
        Err(Refusal::Windows) => {
            let msg = "Windows programs need Wine or Proton; add one to config.json by hand, e.g. \"command\": \"wine game.exe\"";
            return fail(exit::USAGE, msg);
        }
    };
    if let Some(name) = name {
        entry.name = name;
    }
    entry.system = system.filter(|s| s != "PC");
    entry.mode = mode.or(entry.mode);
    if cfg.games.iter().any(|g| g.name.eq_ignore_ascii_case(&entry.name)) {
        return fail(
            exit::USAGE,
            format!("there's already a game called {:?}; pick another name with --name", entry.name),
        );
    }
    if print {
        println!("{}", entry.to_json());
        return Ok(());
    }

    let games_path = cfg_path.parent().unwrap_or(Path::new(".")).join(GAMES_FILE);
    let existing_games = std::fs::read_to_string(&games_path).ok();
    let Some(games_text) = crate::add::append(existing_games.as_deref(), &entry) else {
        let msg = format!(
            "couldn't add to {} safely; add this to its games by hand:\n  {}",
            config::tilde(&games_path),
            entry.to_json()
        );
        return fail(exit::CONFIG, msg);
    };
    let Some(config_text) = crate::init::add_include(&raw, &own, GAMES_FILE, "Games added with `romi add`.") else {
        let msg = format!(
            "couldn't add \"{GAMES_FILE}\" to the \"include\" list in {}; add it by hand",
            config::tilde(cfg_path)
        );
        return fail(exit::CONFIG, msg);
    };
    println!("{}\n  {}", config::tilde(&games_path), entry.to_json());
    let also =
        if config_text == raw { String::new() } else { format!(", and include it from {}", config::tilde(cfg_path)) };
    if !confirm(&format!("\nAdd this to {GAMES_FILE}{also}?"), yes)? {
        return Ok(());
    }
    std::fs::write(&games_path, games_text)?;
    if config_text != raw {
        std::fs::write(cfg_path, config_text)?;
    }
    load_config(cfg_path)?;
    println!("Added {}. Try it with: romi play {}", entry.name, shell_words::quote(&entry.name));
    Ok(())
}

/// `romi` with no config: sets one up as `romi init` does, before the launcher opens. False if
/// nothing was written.
pub fn first_run(cfg_path: &Path) -> Result<bool> {
    use std::io::IsTerminal;
    if cfg_path.exists() {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return load_config(cfg_path).map(|_| false);
    }
    if !std::io::stdout().is_terminal() {
        println!("Welcome to romi! There's no config yet, so let's find your games and emulators.\n");
        let own_folders = !crate::init::retroarch_here();
        init(cfg_path, None, false, false, None, false, Some(FirstRun { own_folders }))?;
        return Ok(cfg_path.exists());
    }
    let found = crate::init::find_roms_dir(None, None);
    let usual = crate::init::usual_roms_dirs();
    // Only asked about if there isn't a login already.
    let account = crate::emucfg::retroarch_login().filter(|_| crate::emu::cheevos::saved_login().is_none());
    let name = account.as_ref().map(|l| l.username.as_str());
    let welcome =
        crate::tui::welcome::first_run(found.as_deref(), &usual, crate::init::retroarch_here(), name, |roms, own| {
            crate::init::first_look(roms, own)
        });
    let Some(answers) = welcome? else {
        return Ok(false);
    };
    let first_run = FirstRun { own_folders: answers.own_folders };
    init(cfg_path, answers.roms, false, true, None, false, Some(first_run))?;
    if let Some(login) = account.filter(|_| answers.use_account) {
        // Not worth stopping for: it can be done again from Options.
        if let Err(e) = crate::emu::cheevos::save_login(&login) {
            eprintln!("romi: couldn't save the RetroAchievements login: {e:#}");
        }
    }
    Ok(cfg_path.exists())
}

/// What a first run has settled before `init`.
pub(super) struct FirstRun {
    /// BIOS files and saves go in romi's own folder, not RetroArch's.
    pub own_folders: bool,
}

pub(super) fn init(
    cfg_path: &Path,
    roms: Option<PathBuf>,
    print: bool,
    yes: bool,
    esde: Option<PathBuf>,
    no_esde: bool,
    first_run: Option<FirstRun>,
) -> Result<()> {
    let existing = crate::init::existing_config(cfg_path);
    if cfg_path.exists() && existing.is_none() {
        return fail(exit::CONFIG, format!("{} can't be read; fix it first", config::tilde(cfg_path)));
    }
    let esde = esde.or_else(|| existing.as_ref().and_then(|(_, c)| c.esde_dir.as_deref()).map(config::expand_path));
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
    if let Some(dir) = &roms
        && !dir.is_dir()
    {
        return fail(exit::NOT_FOUND, format!("{} isn't a folder", dir.display()));
    }
    let found = crate::init::find_roms_dir(roms.as_deref(), existing_cfg);
    let roms_dir = match &found {
        Some(dir) => dir.clone(),
        // A launcher with no ROMs is still one for Steam and for games added with `romi add`.
        None if first_run.is_some() => config::expand_path("~/ROMs"),
        None => {
            let configured = existing_cfg.map(|c| format!("{} (your roms_dir) isn't a folder, and ", c.roms_dir));
            let msg = format!(
                "{}couldn't find your ROMs in {}; say where with --roms DIR",
                configured.unwrap_or_default(),
                crate::init::usual_roms_dirs()
            );
            return fail(exit::NOT_FOUND, msg);
        }
    };
    // The ROMs are somewhere other than the config says. If what it names is gone, point roms_dir
    // at them; if not, it may hold your own systems, so the ones found here get full paths.
    let configured = existing_cfg.map(|c| config::expand_path(&c.roms_dir));
    let elsewhere = configured.as_ref().is_some_and(|c| *c != roms_dir);
    let move_roms_dir = elsewhere && !configured.as_ref().is_some_and(|c| c.is_dir());
    let mut report = crate::init::scan(&catalog, &roms_dir, existing_cfg);
    report.absolute = elsewhere && !move_roms_dir;
    let generated = crate::init::render(&report);
    if print {
        eprint!("{}", crate::init::summary(&report));
        print!("{generated}");
        return Ok(());
    }
    if found.is_some() {
        print!("{}", crate::init::summary(&report));
    } else {
        println!(
            "No ROM folders found in {}. romi will start with your Steam games and anything you add\n\
             with `romi add`. Once you have ROMs, put them in ~/ROMs/<system> and run `romi init`.",
            crate::init::usual_roms_dirs()
        );
    }
    let already_included =
        existing.as_ref().is_some_and(|(_, cfg)| cfg.include.iter().any(|i| i == crate::init::INCLUDE_FILE));
    if existing.is_some()
        && !already_included
        && !move_roms_dir
        && !report.findings.iter().any(|f| matches!(f, crate::init::Finding::New { .. }))
    {
        println!("\nNothing new to add, so your config is unchanged.");
        return Ok(());
    }

    let include_path = cfg_path.parent().unwrap_or(Path::new(".")).join(crate::init::INCLUDE_FILE);
    let own_folders = match &first_run {
        Some(first_run) => first_run.own_folders,
        None => existing.is_none() && !crate::init::retroarch_here(),
    };
    let (main, plan) = match &existing {
        Some((raw, cfg)) => {
            let shown = config::tilde(cfg_path);
            let mut text = raw.clone();
            let mut changes = Vec::new();
            if move_roms_dir {
                let dir = config::tilde(&roms_dir);
                let Some(edited) = crate::init::set_roms_dir(&text, &dir) else {
                    let msg =
                        format!("couldn't set \"roms_dir\" to {dir:?} in {shown}; set it by hand, then run init again");
                    return fail(exit::CONFIG, msg);
                };
                text = edited;
                changes.push(format!("point roms_dir at {dir}"));
            }
            let comment = "Systems found by `romi init`. Anything set in this file takes priority.";
            let Some(edited) = crate::init::add_include(&text, cfg, crate::init::INCLUDE_FILE, comment) else {
                let msg = format!(
                    "couldn't add \"{}\" to the \"include\" list in {shown}; add it by hand, then run init again",
                    crate::init::INCLUDE_FILE,
                );
                return fail(exit::CONFIG, msg);
            };
            if edited != text {
                changes.insert(0, "include it".to_string());
            }
            let mut plan = format!("write {}", config::tilde(&include_path));
            if !changes.is_empty() {
                plan += &format!(", and in {shown} {}", changes.join(" and "));
            }
            ((edited != *raw).then_some(edited), plan)
        }
        None => {
            let mut plan = format!("write {} and a new {}", config::tilde(&include_path), config::tilde(cfg_path));
            if own_folders {
                let folder = config::tilde(&config::data_dir());
                plan = format!("{plan}, and make {folder} for your BIOS files, saves and cores");
            }
            (Some(crate::init::fresh_config(&roms_dir, own_folders)), plan)
        }
    };
    if !confirm(&format!("\nThis will {plan}. Go ahead?"), yes)? {
        return Ok(());
    }
    if let Some(dir) = cfg_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&include_path, generated)?;
    if let Some(text) = main {
        std::fs::write(cfg_path, text)?;
    }
    let folders = match (own_folders && existing.is_none(), existing.is_none()) {
        (true, _) => Some(crate::init::make_own_folders()?),
        (false, true) => Some(crate::init::make_retroarch_system_folder()?),
        (false, false) => None,
    };
    // Catch anything that would stop romi from starting now, rather than the next time it does.
    let cfg = load_config(cfg_path)?;
    let lib = Library::load(&cfg);
    let done = format!("{} games in {} systems", lib.games.len(), lib.systems.len());
    if first_run.is_some() {
        println!("\nDone: {done}. `romi check` shows what was found, and what could fix anything missing.");
    } else {
        println!("\nDone: {done}. Run `romi` to browse them, or `romi check` for details.");
    }
    if let Some(folders) = folders {
        println!("{folders}");
    }
    Ok(())
}

fn print_suggestions(cfg: &Config, lib: &Library) {
    use crate::init::{Suggestion, describe};
    let catalog = crate::init::Catalog::for_config(cfg);
    let suggestions = crate::init::suggest(&catalog, cfg, lib);
    if suggestions.is_empty() {
        return;
    }
    let games = |n: usize| format!("{n} {}", text::plural(n, "game"));
    let missing = suggestions.iter().any(|s| matches!(s, Suggestion::Install { .. }));
    println!("\nsuggestions");
    for s in suggestions {
        match s {
            Suggestion::Replace { id, run } => {
                println!("  {id:<12} {} is installed and can run it. To use it, set its command to", describe(&run));
                println!("  {:<12} {}", "", serde_json::to_string(&run.command).unwrap_or_default());
            }
            Suggestion::Add { folder, name, games: n, run } => println!(
                "  {folder:<12} {} for {name}, not in your config. {} can run them: `romi init` adds them.",
                games(n),
                describe(&run)
            ),
            Suggestion::Install { folder, name, games: n, tried } => {
                let known = crate::init::what_would_run(&tried, catalog.esde);
                println!("  {folder:<12} {} for {name}, but no emulator for it is installed{known}", games(n));
            }
        }
    }
    if missing {
        println!("\n{}", crate::init::where_cores_come_from());
    }
}

/// "34 in ~/cores, 17 in /usr/lib/libretro", or where it looked if it found none.
fn where_cores_are(folders: &[(std::path::PathBuf, usize)]) -> String {
    let found: Vec<String> =
        folders.iter().filter(|(_, n)| *n > 0).map(|(dir, n)| format!("{n} in {}", config::tilde(dir))).collect();
    if !found.is_empty() {
        return found.join(", ");
    }
    let looked: Vec<String> = folders.iter().map(|(dir, _)| config::tilde(dir)).collect();
    format!("! none found, looked in {}", looked.join(", "))
}

/// The system folder cores get, or where it should be if there's none.
fn where_bios_is(cfg: &Config) -> String {
    match (cfg.system_dir(), cfg.bios_dirs.first()) {
        (Some(dir), _) => config::tilde(&dir),
        (None, Some(dir)) => format!("! {dir} doesn't exist, so cores have no system folder"),
        (None, None) => "! no \"bios_dirs\" set, so cores have no system folder".to_string(),
    }
}

/// Where in-game saves go, for games played in romi.
fn where_saves_are(cfg: &Config) -> String {
    match &cfg.saves_dir {
        Some(dir) => dir.clone(),
        // With no game or core, the folder RetroArch sorts them into.
        None => match crate::emucfg::retroarch_save_dir("", Path::new("")) {
            Some(dir) => config::tilde(&dir),
            None => "next to each game, as RetroArch has them".to_string(),
        },
    }
}

pub(super) fn check(cfg: &Config, cfg_path: &std::path::Path) -> Result<()> {
    let lib = Library::load_complete(cfg);
    println!("config    {}", config::tilde(cfg_path));
    println!("roms      {}", cfg.roms_dir);
    let terminal = crate::emu::kitty::terminal();
    match terminal.shows_games {
        true => println!("terminal  {}: games play inside romi", terminal.name),
        false => println!(
            "terminal  ! {}: games only show inside romi in Ghostty or kitty on this machine, so A starts their emulators",
            terminal.name
        ),
    }
    if terminal.name == "Ghostty" && crate::emucfg::ghostty_blur() {
        println!("          ! background-blur in Ghostty's config adds lag to games: about 45 ms measured at 4K");
    }
    let core_folders = crate::init::Catalog::for_config(cfg).core_folders();
    println!("cores     {}", where_cores_are(&core_folders));
    println!("bios      {}", where_bios_is(cfg));
    println!("saves     {}", where_saves_are(cfg));
    println!("states    {}\n", config::tilde(&config::data_dir().join("states")));
    let roms = config::expand_path(&cfg.roms_dir);
    let width = lib.systems.iter().map(|s| s.name.chars().count()).max().unwrap_or(0).max(24);
    let games = |n: usize| format!("{n:>4} {:<5}", text::plural(n, "game"));
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
        println!("  {mark} {:<width$} {}", cfg.system_name(sys), detail.trim_end());
    }
    for s in &lib.systems {
        if !cfg.systems.iter().any(|c| c.id == s.id) {
            println!("  ✓ {:<width$} {}", s.name, games(s.games.len()).trim_end());
        }
    }
    let emulators = crate::emucfg::check(cfg, &lib);
    if !emulators.is_empty() {
        let width = emulators.iter().map(|e| e.name.chars().count()).max().unwrap_or(0);
        println!("\nemulators");
        for e in &emulators {
            for note in &e.notes {
                println!("  {} {:<width$}  {}", if note.ok { "✓" } else { "!" }, e.name, note.text);
            }
        }
    }
    print_suggestions(cfg, &lib);
    // PCSX2's BIOS doesn't count: LRPS2 only looks in the folder it has in romi's system folder.
    let has_lrps2 = core_folders.iter().any(|(dir, _)| dir.join("pcsx2_libretro.so").is_file());
    let has_ps2_games = lib.systems.iter().any(|s| s.id == "ps2" && !s.games.is_empty());
    let lrps2_bios = [config::BiosFile::One("pcsx2/bios/*.bin".into())];
    if has_lrps2 && has_ps2_games && !config::missing_bios(&lrps2_bios, &cfg.bios_dirs).is_empty() {
        let folder = cfg.bios_dirs.first().map_or("your system folder", String::as_str);
        println!("\n  ! LRPS2 has no BIOS in {folder}/pcsx2/bios, so PS2 games won't start in romi");
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
