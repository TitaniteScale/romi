//! The controls screen: which of the pad's buttons sends each of a game's buttons when it's played
//! in romi, for the one game or for all its system's. Pick a button and press the one you want for
//! it, and the two swap. Changes are saved straight away: a game's in `overrides.jsonc`, a
//! system's in the config's `"controls"`, over RetroArch's remap for the game, if it has one.
//!
//! The buttons are named as the core names them, which it only says as a game starts, so romi
//! remembers the names from the last time the game, or one of its system's, was played.

use crate::config::Config;
use crate::emu::libretro::BUTTONS;
use crate::emu::remap::{Controls, Remap};
use crate::library::Library;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The buttons in the order they're listed, when the core hasn't named them: the d-pad, the face
/// buttons, the shoulders, then Select and Start.
const ORDER: [&str; 16] =
    ["up", "down", "left", "right", "a", "b", "x", "y", "l", "r", "l2", "r2", "l3", "r3", "select", "start"];

/// What libretro's pad calls its buttons, for a core that doesn't say.
const PAD_NAMES: [(&str, &str); 16] = [
    ("up", "D-pad up"),
    ("down", "D-pad down"),
    ("left", "D-pad left"),
    ("right", "D-pad right"),
    ("a", "A"),
    ("b", "B"),
    ("x", "X"),
    ("y", "Y"),
    ("l", "L"),
    ("r", "R"),
    ("l2", "L2"),
    ("r2", "R2"),
    ("l3", "L3"),
    ("r3", "R3"),
    ("select", "Select"),
    ("start", "Start"),
];

/// What's printed on a controller's buttons, the way romi's hints name them: A at the bottom.
const CONTROLLER_LABELS: [(&str, &str); 16] = [
    ("up", "↑"),
    ("down", "↓"),
    ("left", "←"),
    ("right", "→"),
    ("a", "B"),
    ("b", "A"),
    ("x", "Y"),
    ("y", "X"),
    ("l", "LB"),
    ("r", "RB"),
    ("l2", "LT"),
    ("r2", "RT"),
    ("l3", "LS"),
    ("r3", "RS"),
    ("select", "Select"),
    ("start", "Start"),
];

fn place(name: &str) -> Option<usize> {
    BUTTONS.iter().position(|b| *b == name)
}

fn lookup(table: &[(&str, &'static str)], place: usize) -> &'static str {
    table.iter().find(|(n, _)| *n == BUTTONS[place]).map_or("?", |(_, label)| *label)
}

/// The pad's button at `place` as you'd press it: on the controller, or on the keyboard.
pub fn pressed_label(place: usize, pad: bool) -> &'static str {
    match pad {
        true => lookup(&CONTROLLER_LABELS, place),
        false => crate::emu::controls::KEYS.iter().find(|(n, _)| *n == BUTTONS[place]).map_or("—", |(_, k)| *k),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// Whether the changes are for the game or its whole system.
    For,
    /// One of the game's buttons, by its place in `BUTTONS`, and the pad's buttons that send it.
    Button(usize),
    /// Back to the remap below.
    Reset,
}

pub struct Screen {
    pub game: usize,
    /// For all the system's games, rather than just this one.
    pub for_system: bool,
    pub selected: usize,
    /// Waiting for the button to press for the selected one.
    pub capturing: bool,
    /// The core's name, which finds RetroArch's remap.
    core: Option<String>,
    rom: PathBuf,
    /// What the core calls the game's buttons, in the order they're listed.
    names: Vec<(usize, String)>,
}

impl Screen {
    pub fn new(lib: &Library, g: usize, core: Option<String>, rom: PathBuf) -> Screen {
        let mut names = remembered(&lib.games[g].key, &lib.system_of(g).id);
        if names.is_empty() {
            names = (0..BUTTONS.len()).map(|p| (p, lookup(&PAD_NAMES, p).to_string())).collect();
        }
        names.sort_by_key(|(p, _)| ORDER.iter().position(|n| *n == BUTTONS[*p]));
        Screen { game: g, for_system: false, selected: 1, capturing: false, core, rom, names }
    }

    pub fn rows(&self) -> Vec<Row> {
        [Row::For].into_iter().chain(self.names.iter().map(|(p, _)| Row::Button(*p))).chain([Row::Reset]).collect()
    }

    pub fn row(&self) -> Row {
        self.rows().get(self.selected).copied().unwrap_or(Row::For)
    }

    /// What the game calls the button at `place`.
    pub fn name(&self, place: usize) -> &str {
        self.names.iter().find(|(p, _)| *p == place).map_or("?", |(_, n)| n.as_str())
    }

    /// The remap below the one being changed (RetroArch's, and for a game, its system's over
    /// that), and the one being changed, as the game plays with it now.
    pub fn remaps(&self, cfg: &Config, lib: &Library) -> (Remap, Remap) {
        let retroarch = crate::emu::runner::remap(self.core.as_deref(), &self.rom, &[]);
        let system = cfg.controls.get(&lib.system_of(self.game).id);
        match self.for_system {
            true => (retroarch.clone(), retroarch.with(system.unwrap_or(&Controls::default()))),
            false => {
                let below = retroarch.with(system.unwrap_or(&Controls::default()));
                (below.clone(), below.with(&lib.games[self.game].controls))
            }
        }
    }

    /// The controls to save after `pressed` (a pad button's name) is pressed for the selected
    /// button, if it's one.
    pub fn press(&self, cfg: &Config, lib: &Library, pressed: &str) -> Option<Controls> {
        let (Row::Button(wanted), Some(pressed)) = (self.row(), place(pressed)) else { return None };
        let (below, mut now) = self.remaps(cfg, lib);
        now.swap(wanted, pressed);
        Some(now.controls_over(&below))
    }
}

// ---- the buttons' names, remembered ----

/// What cores called games' buttons, the last time each game, and one of each system's, was played.
#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct Names {
    games: BTreeMap<String, Vec<(usize, String)>>,
    systems: BTreeMap<String, Vec<(usize, String)>>,
}

fn names_path() -> PathBuf {
    crate::config::state_dir().join("buttons.json")
}

fn read_names() -> Names {
    std::fs::read_to_string(names_path()).ok().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default()
}

/// Remembers what the core that played a game called its buttons.
pub fn remember(key: &str, system: &str, names: Vec<(usize, String)>) {
    if names.is_empty() {
        return;
    }
    let mut all = read_names();
    if all.games.get(key) == Some(&names) && all.systems.get(system) == Some(&names) {
        return;
    }
    all.games.insert(key.to_string(), names.clone());
    all.systems.insert(system.to_string(), names);
    let path = names_path();
    if let (Some(dir), Ok(json)) = (path.parent(), serde_json::to_string(&all)) {
        let _ = std::fs::create_dir_all(dir);
        let _ = crate::files::write_atomic(&path, json);
    }
}

/// The game's buttons' names, or its system's, from when one was last played.
fn remembered(key: &str, system: &str) -> Vec<(usize, String)> {
    let mut all = read_names();
    let names = all.games.remove(key).or_else(|| all.systems.remove(system)).unwrap_or_default();
    names.into_iter().filter(|(p, _)| *p < BUTTONS.len()).collect()
}
