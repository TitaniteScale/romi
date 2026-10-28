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
    println!("Welcome to romi! There's no config yet, so let's find your games and emulators.\n");
    init(cfg_path, None, false, false, None, false, true)?;
    Ok(cfg_path.exists())
}

pub(super) fn init(
    cfg_path: &Path,
    roms: Option<PathBuf>,
    print: bool,
    yes: bool,
    esde: Option<PathBuf>,
    no_esde: bool,
    first_run: bool,
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
        None if first_run => config::expand_path("~/ROMs"),
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
            let plan = format!("write {} and a new {}", config::tilde(&include_path), config::tilde(cfg_path));
            (Some(crate::init::fresh_config(&roms_dir)), plan)
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
    // Catch anything that would stop romi from starting now, rather than the next time it does.
    let cfg = load_config(cfg_path)?;
    let lib = Library::load(&cfg);
    let done = format!("{} games in {} systems", lib.games.len(), lib.systems.len());
    if first_run {
        println!("\nDone: {done}. `romi check` shows what was found, and what could fix anything missing.");
    } else {
        println!("\nDone: {done}. Run `romi` to browse them, or `romi check` for details.");
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
                let known: Vec<&str> = tried.iter().map(|t| t.split(':').next().unwrap_or(t)).take(4).collect();
                let known =
                    if known.is_empty() { String::new() } else { format!(" (ES-DE knows: {})", known.join(", ")) };
                println!("  {folder:<12} {} for {name}, but no emulator for it is installed{known}", games(n));
            }
        }
    }
}

pub(super) fn check(cfg: &Config, cfg_path: &std::path::Path) -> Result<()> {
    let lib = Library::load_complete(cfg);
    println!("config  {}", config::tilde(cfg_path));
    println!("roms    {}\n", cfg.roms_dir);
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
