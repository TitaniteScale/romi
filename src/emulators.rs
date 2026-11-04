//! Other emulators a game can run with: the ones its system lists under `"emulators"`, then the
//! installed ones ES-DE knows for the system. A game's menu picks one, saved as an override.

use crate::config::{Config, Emulator};
use crate::init::Catalog;
use crate::launch::Launch;
use crate::library::{Game, Library};
use std::collections::HashMap;
use std::path::PathBuf;

/// The other emulators for each system, looked up once per system since finding the installed
/// ones means reading ES-DE's definitions and looking for programs.
#[derive(Default)]
pub struct Emulators {
    catalog: Option<Catalog>,
    by_system: HashMap<String, Vec<Emulator>>,
}

impl Emulators {
    /// Looks up the emulators for a game's system, unless that's been done.
    pub fn find(&mut self, cfg: &Config, lib: &Library, g: usize) {
        let id = &lib.system_of(g).id;
        if self.by_system.contains_key(id) {
            return;
        }
        let mut found: Vec<Emulator> = Vec::new();
        // Only ROMs: hand-added games and Steam have no system command to swap.
        if let Some(conf) = cfg.systems.iter().find(|s| &s.id == id).filter(|_| lib.games[g].path.is_some()) {
            found.extend(conf.emulators.iter().cloned());
            let catalog = self.catalog.get_or_insert_with(|| Catalog::for_config(cfg));
            for t in catalog.installed_emulators(cfg, conf) {
                let name = crate::init::describe(&t);
                if !found.iter().any(|e| e.name == name || e.command == t.command) {
                    found.push(Emulator { name, command: t.command, mode: t.mode, cwd: t.cwd });
                }
            }
        }
        self.by_system.insert(id.clone(), found);
    }

    /// The ways a game can run, if `find` has looked them up: how it runs by default first
    /// (named after the emulator it is, when that's one of the others), then the others.
    pub fn options(&self, lib: &Library, g: usize) -> Vec<Emulator> {
        let game = &lib.games[g];
        let Some(others) = self.by_system.get(&lib.system_of(g).id).filter(|o| !o.is_empty()) else {
            return Vec::new();
        };
        let argv = |command: &str| {
            Launch { command: command.to_string(), ..game.launch.clone() }.prepare().ok().map(|p| p.argv)
        };
        let default_argv = argv(&game.default_run.command);
        let is_default = |e: &&Emulator| argv(&e.command) == default_argv;
        let mut default = game.default_run.clone();
        default.name = match others.iter().find(is_default) {
            Some(e) => e.name.clone(),
            None => runs_label(&default.command),
        };
        let mut options = vec![default];
        options.extend(others.iter().filter(|e| !is_default(e)).cloned());
        options
    }
}

/// The libretro core a game plays in romi with: the one its own command runs, or else the first
/// installed one among the other ways it can run (`options`), so a DS game that runs in
/// standalone melonDS still plays in romi through RetroArch's melonDS core.
pub fn core(game: &Game, options: &[Emulator]) -> Option<PathBuf> {
    let core = |command: &str| {
        let argv = Launch { command: command.to_string(), ..game.launch.clone() }.prepare().ok()?.argv;
        crate::emu::libretro::core_in(&argv).filter(|core| core.is_file())
    };
    core(&game.launch.command).or_else(|| options.iter().find_map(|e| core(&e.command)))
}

/// The core and ROM to play a game inside romi with, if it has a core that can open the ROM.
pub fn playable(game: &Game, options: &[Emulator]) -> Option<(PathBuf, PathBuf)> {
    let rom = game.path.clone()?;
    let core = core(game, options)?;
    crate::emu::libretro::can_open(&core, &rom).then_some((core, rom))
}

/// Which of `options` the game uses now.
pub fn current(game: &Game, options: &[Emulator]) -> usize {
    let picked = game.emulator.as_deref();
    picked.and_then(|name| options.iter().skip(1).position(|e| e.name == name).map(|i| i + 1)).unwrap_or(0)
}

/// "retroarch · mgba" for libretro cores, otherwise just the program name.
pub fn runs_label(command: &str) -> String {
    let mut words = command.split_whitespace();
    let program = words.next().unwrap_or_default();
    let program = program.rsplit('/').next().unwrap_or(program);
    match words.find_map(|w| w.rsplit('/').next()?.strip_suffix("_libretro.so")) {
        Some(core) => format!("{program} · {core}"),
        None => program.to_string(),
    }
}
