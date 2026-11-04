//! What romi can tell from emulators' own settings files: for `romi check`, whether
//! RetroAchievements is on and logged in and whether a controller is bound; for games played in
//! romi, where RetroArch keeps saves (or melonDS and Dolphin, for the games they run), and the core
//! options and remaps RetroArch has. romi only ever reads these files.

use crate::config::{Config, expand_path};
use crate::emu::remap::Remap;
use crate::library::Library;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Emulators whose settings romi knows how to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Emulator {
    RetroArch,
    Pcsx2,
    Dolphin,
    MelonDs,
}

/// System ids Dolphin runs as a GameCube, with controllers on the GameCube ports.
const GAMECUBE: &[&str] = &["gc", "gamecube", "ngc"];

impl Emulator {
    pub fn name(self) -> &'static str {
        match self {
            Emulator::RetroArch => "RetroArch",
            Emulator::Pcsx2 => "PCSX2",
            Emulator::Dolphin => "Dolphin",
            Emulator::MelonDs => "melonDS",
        }
    }

    fn flatpak_id(self) -> &'static str {
        match self {
            Emulator::RetroArch => "org.libretro.RetroArch",
            Emulator::Pcsx2 => "net.pcsx2.PCSX2",
            Emulator::Dolphin => "org.DolphinEmu.dolphin-emu",
            Emulator::MelonDs => "net.kuribo64.melonDS",
        }
    }

    /// The emulator a command runs, and whether it's the Flatpak. The first program named wins, so
    /// RetroArch's melonDS core is RetroArch.
    pub fn of(command: &str) -> Option<(Emulator, bool)> {
        let words = shell_words::split(command).unwrap_or_default();
        let flatpak = words.iter().any(|w| w == "flatpak");
        let found = words.iter().find_map(|w| {
            let base = w.rsplit('/').next().unwrap_or(w);
            let lower = base.to_lowercase();
            [Emulator::RetroArch, Emulator::Pcsx2, Emulator::Dolphin, Emulator::MelonDs].into_iter().find(|e| {
                base == e.flatpak_id()
                    || match e {
                        Emulator::RetroArch => lower == "retroarch",
                        Emulator::Pcsx2 => lower.starts_with("pcsx2"),
                        Emulator::Dolphin => lower.starts_with("dolphin-emu"),
                        Emulator::MelonDs => lower.starts_with("melonds"),
                    }
            })
        })?;
        Some((found, flatpak))
    }

    /// Where it keeps its settings.
    fn config_dir(self, flatpak: bool) -> PathBuf {
        let base = if flatpak {
            expand_path(&format!("~/.var/app/{}/config", self.flatpak_id()))
        } else {
            dirs::config_dir().unwrap_or_else(|| expand_path("~/.config"))
        };
        base.join(match self {
            Emulator::RetroArch => "retroarch",
            Emulator::Pcsx2 => "PCSX2",
            Emulator::Dolphin => "dolphin-emu",
            Emulator::MelonDs => "melonDS",
        })
    }
}

/// One thing found out about an emulator's setup.
#[derive(Debug, PartialEq)]
pub struct Note {
    pub ok: bool,
    pub text: String,
}

fn ok(text: impl Into<String>) -> Option<Note> {
    Some(Note { ok: true, text: text.into() })
}

fn problem(text: impl Into<String>) -> Option<Note> {
    Some(Note { ok: false, text: text.into() })
}

pub struct Report {
    /// "Dolphin", or "Dolphin (Flatpak)".
    pub name: String,
    pub notes: Vec<Note>,
}

/// The emulators that run games in the library, and what their settings say. Achievements are
/// checked when RetroAchievements is set up in romi, controllers when romi's controller support
/// is on.
pub fn check(cfg: &Config, lib: &Library) -> Vec<Report> {
    let mut used: BTreeMap<(Emulator, bool), Vec<&str>> = BTreeMap::new();
    for sys in &cfg.systems {
        if !lib.systems.iter().any(|s| s.id == sys.id && !s.games.is_empty()) {
            continue;
        }
        // The system's command, and the ones picked for its games.
        let prefix = format!("{}:", sys.id);
        let picked = cfg.overrides.iter().filter(|(k, _)| k.starts_with(&prefix)).map(|(_, o)| o);
        let commands = std::iter::once(&sys.command)
            .chain(sys.overrides.values().chain(picked).filter_map(|o| o.command.as_ref()));
        for command in commands {
            if let Some(found) = Emulator::of(command) {
                used.entry(found).or_default().push(&sys.id);
            }
        }
    }
    for game in &cfg.games {
        if let Some(found) = Emulator::of(&game.command) {
            used.entry(found).or_default();
        }
    }
    let user = cfg.retroachievements.username.as_deref().map(str::trim).filter(|u| !u.is_empty());
    used.into_iter()
        .filter_map(|((emu, flatpak), systems)| {
            let dir = emu.config_dir(flatpak);
            let mut notes = Vec::new();
            notes.extend(user.and_then(|u| achievements(emu, &dir, u)));
            if cfg.controller.enabled {
                notes.extend(controller(emu, &dir, &systems));
            }
            let name = if flatpak { format!("{} (Flatpak)", emu.name()) } else { emu.name().to_string() };
            (!notes.is_empty()).then_some(Report { name, notes })
        })
        .collect()
}

fn achievements(emu: Emulator, dir: &Path, user: &str) -> Option<Note> {
    let (on, who, logged_in, place) = match emu {
        Emulator::RetroArch => {
            let ini = Ini::read(&dir.join("retroarch.cfg"));
            let who = ini.get("", "cheevos_username").unwrap_or_default();
            // It trades the password for a token when it logs in.
            let signed = ini.has("", "cheevos_token") || (!who.is_empty() && ini.has("", "cheevos_password"));
            (ini.get("", "cheevos_enable") == Some("true"), who.to_string(), signed, "Settings → Achievements")
        }
        Emulator::Pcsx2 => {
            // The token is kept in a separate secrets file; the username says it logged in.
            let ini = Ini::read(&dir.join("inis/PCSX2.ini"));
            let who = ini.get("Achievements", "Username").unwrap_or_default();
            let on = ini.get("Achievements", "Enabled") == Some("true");
            (on, who.to_string(), !who.is_empty(), "Settings → Achievements")
        }
        Emulator::Dolphin => {
            let ini = Ini::read(&dir.join("RetroAchievements.ini"));
            let who = ini.get("Achievements", "Username").unwrap_or_default();
            let on = ini.get("Achievements", "Enabled").is_some_and(|v| v.eq_ignore_ascii_case("true"));
            (on, who.to_string(), ini.has("Achievements", "ApiToken"), "Tools → Achievements")
        }
        Emulator::MelonDs => return None,
    };
    match (on, logged_in) {
        (false, false) => problem(format!("RetroAchievements is off and not logged in ({place})")),
        (false, true) => problem(format!("RetroAchievements is off ({place})")),
        (true, false) => problem(format!("not logged in to RetroAchievements ({place})")),
        (true, true) if !who.is_empty() && !who.eq_ignore_ascii_case(user) => {
            problem(format!("logged in to RetroAchievements as {who}, but romi shows {user}'s progress"))
        }
        (true, true) => {
            ok(format!("RetroAchievements is on, logged in as {}", if who.is_empty() { user } else { &who }))
        }
    }
}

fn controller(emu: Emulator, dir: &Path, systems: &[&str]) -> Option<Note> {
    match emu {
        // Its autoconfig profiles set up most controllers by themselves.
        Emulator::RetroArch => None,
        Emulator::MelonDs => {
            let path = dir.join("melonDS.toml");
            // melonDS before 1.0 kept its settings in another format.
            if !path.exists() && dir.join("melonDS.ini").exists() {
                return None;
            }
            let ini = Ini::read(&path);
            let bound = ini.section("Instance0.Joystick").any(|(_, v)| v.parse::<i64>().is_ok_and(|n| n != -1));
            if bound {
                ok("a controller is bound")
            } else {
                problem("no controller buttons are bound (Config → Input and hotkeys)")
            }
        }
        Emulator::Pcsx2 => {
            let ini = Ini::read(&dir.join("inis/PCSX2.ini"));
            if ini.get("Pad1", "Type") == Some("None") {
                return None;
            }
            let pad = |v: &str| ["SDL-", "XInput-", "DInput-"].iter().any(|p| v.contains(p));
            if ini.section("Pad1").any(|(_, v)| pad(v)) {
                ok("a controller is bound to port 1")
            } else {
                problem("no controller is bound to port 1 (Settings → Controllers)")
            }
        }
        Emulator::Dolphin => {
            if !systems.iter().any(|s| GAMECUBE.contains(s)) {
                return None;
            }
            // Port 1 set to anything but an emulated controller (a GameCube adapter, say) doesn't
            // use these bindings.
            let core = Ini::read(&dir.join("Dolphin.ini"));
            if core.get("Core", "SIDevice0").is_some_and(|d| d != "6") {
                return None;
            }
            let pads = Ini::read(&dir.join("GCPadNew.ini"));
            let device = pads.get("GCPad1", "Device").unwrap_or_default();
            if ["SDL/", "evdev/", "DSUClient/"].iter().any(|p| device.starts_with(p)) {
                return ok("a controller is on GameCube port 1");
            }
            let what = if device.is_empty() || device.starts_with("XInput2/") { "the keyboard" } else { device };
            problem(format!("GameCube port 1 uses {what}, not a controller (Controllers → Port 1 → Configure)"))
        }
    }
}

/// The folder RetroArch gives the core called `core` (its name without the version) for what it
/// writes while playing `rom`, so a game played in romi shares those files with RetroArch. None
/// when RetroArch keeps them next to the game, which is also where cores go without a folder.
pub fn retroarch_save_dir(core: &str, rom: &Path) -> Option<PathBuf> {
    let dir = Emulator::RetroArch.config_dir(false);
    save_dir(&Ini::read(&dir.join("retroarch.cfg")), &dir, core, rom)
}

/// The folder RetroArch gives cores for BIOS files, from its settings: `~/.config/retroarch/system`
/// unless they name another.
pub fn retroarch_system_dir() -> PathBuf {
    let dir = Emulator::RetroArch.config_dir(false);
    system_dir(&Ini::read(&dir.join("retroarch.cfg")), &dir)
}

fn system_dir(ini: &Ini, config_dir: &Path) -> PathBuf {
    match ini.get("", "system_directory") {
        // RetroArch's own words for its default, and a folder relative to itself.
        None | Some("" | "default") => config_dir.join("system"),
        Some(dir) if dir.starts_with(':') => config_dir.join("system"),
        Some(dir) => expand_path(dir),
    }
}

/// Saves a libretro core shares with the emulator a game runs in outside romi.
#[derive(Debug, PartialEq)]
pub enum SharedSaves {
    /// The core keeps the game's save in this folder, as the emulator does.
    Folder(PathBuf),
    /// Dolphin's core keeps memory cards in `User/GC` under the folder it's given, as standalone
    /// Dolphin keeps them under its own: this is standalone Dolphin's `GC` folder, for the core's
    /// to be.
    DolphinGc(PathBuf),
}

/// What the emulator `command` runs keeps of `rom`'s saves, when the libretro core called `core`
/// (its name without the version) keeps them just as it does, so a game played in romi carries on
/// outside it and back: DS games that run in standalone melonDS, played with RetroArch's melonDS
/// core (both keep `<name>.sav`, the same inside), and GameCube games that run in standalone
/// Dolphin, played with RetroArch's Dolphin core (both keep memory cards as GCI folders).
pub fn shared_saves(command: &str, core: &str, rom: &Path) -> Option<SharedSaves> {
    match Emulator::of(command)? {
        (Emulator::MelonDs, flatpak) if core == "melonDS" => {
            melonds_save_dir(&Emulator::MelonDs.config_dir(flatpak), rom).map(SharedSaves::Folder)
        }
        (Emulator::Dolphin, flatpak) if core == "dolphin-emu" => {
            let data = match flatpak {
                true => expand_path(&format!("~/.var/app/{}/data/dolphin-emu", Emulator::Dolphin.flatpak_id())),
                false => dirs::data_dir().unwrap_or_else(|| expand_path("~/.local/share")).join("dolphin-emu"),
            };
            dolphin_gc_dir(&Emulator::Dolphin.config_dir(flatpak), &data, &expand_path("~/.dolphin-emu"))
                .map(SharedSaves::DolphinGc)
        }
        _ => None,
    }
}

/// Dolphin's `MemoryCardFolder` device, a GCI folder, as Slot A is by default.
const GCI_FOLDER: &str = "8";

/// Where standalone Dolphin keeps GameCube memory cards (its `GC` folder), with its settings in
/// `config_dir` and the rest in `data_dir`, or both in `legacy` if that's there, as Dolphin looks
/// for it first. None unless Slot A is a GCI folder in the usual place, as the core's is: a card
/// in one file, or a folder of its own, wouldn't be found by the core.
fn dolphin_gc_dir(config_dir: &Path, data_dir: &Path, legacy: &Path) -> Option<PathBuf> {
    let (config, data) = match legacy.is_dir() {
        true => (legacy.join("Config"), legacy.to_path_buf()),
        false => (config_dir.to_path_buf(), data_dir.to_path_buf()),
    };
    let ini = Ini::read(&config.join("Dolphin.ini"));
    let set = |key| ini.get("Core", key).filter(|v| !v.is_empty());
    (set("SlotA").is_none_or(|slot| slot == GCI_FOLDER) && set("GCIFolderAPathOverride").is_none())
        .then(|| data.join("GC"))
}

/// Where melonDS keeps `rom`'s save: next to it, unless its settings name a folder. A relative
/// folder is relative to wherever melonDS was started, which romi can't know.
fn melonds_save_dir(config_dir: &Path, rom: &Path) -> Option<PathBuf> {
    let toml = config_dir.join("melonDS.toml");
    // melonDS before 1.0 kept its settings in another format, with the same name for this.
    let set = match toml.exists() {
        true => Ini::read(&toml).get("Instance0", "SaveFilePath").map(str::to_string),
        false => Ini::read(&config_dir.join("melonDS.ini")).get("", "SaveFilePath").map(str::to_string),
    };
    match set.filter(|dir| !dir.is_empty()).map(|dir| expand_path(&dir)) {
        None => rom.parent().map(Path::to_path_buf),
        Some(dir) => dir.is_absolute().then_some(dir),
    }
}

/// What games are known to need from a core, by the core's name and the game's serial, with what
/// was seen. These go under everything RetroArch has, so any option file of yours beats them.
struct KnownGame {
    /// The serial on the disc (`SYSTEM.CNF`'s boot file, like `SCUS_972.11`).
    serial: &'static str,
    options: &'static [(&'static str, &'static str)],
}

/// LRPS2's names: the current one, and the one it had.
const LRPS2: [&str; 2] = ["ParaLLEl PS2", "LRPS2"];

const LRPS2_GAMES: &[KnownGame] = &[
    // ATV Offroad Fury 2 (USA) reads the GPU back three times a frame, each time waiting for it:
    // at 2× on a Radeon 890M, 52 fps and the sound dry 56 times in 19 s. With readbacks off, 59.8
    // fps and never. Only tried from a cold boot into one race, on one machine, and not known to
    // matter to anything in the game. Other regions' serials aren't here because none were tried.
    KnownGame { serial: "SCUS_972.11", options: &[("pcsx2_hw_download_mode", "Disable Readbacks")] },
];

/// Beetle PSX's DualShock (`RETRO_DEVICE_SUBCLASS(RETRO_DEVICE_ANALOG, 1)`, which the core lists
/// as "DualShock" with `SET_CONTROLLER_INFO`).
const BEETLE_DUALSHOCK: u32 = 517;

/// PlayStation games that can't be played with the plain pad Beetle PSX plugs in, by the start of
/// their file names. (A DualShock isn't plugged in for every game: with the sticks listed, romi
/// no longer makes the left stick a d-pad.) Matched by name since most discs are `.chd`, whose
/// serial can't be read.
const DUALSHOCK_GAMES: &[&str] = &[
    // Its levels are played with both sticks.
    "ape escape",
];

/// The controller `core` is known to want plugged into port 1 for `rom`: what RetroArch's remaps
/// would say, which go over this.
pub fn known_device(core: &str, rom: &Path) -> Option<u32> {
    if !core.to_ascii_lowercase().starts_with("beetle psx") {
        return None;
    }
    let name = rom.file_stem()?.to_string_lossy().to_ascii_lowercase();
    DUALSHOCK_GAMES.iter().any(|game| name.starts_with(game)).then_some(BEETLE_DUALSHOCK)
}

/// The options `core` is known to want for `rom`, if it's one of the games above.
fn known_options(core: &str, rom: &Path) -> BTreeMap<String, String> {
    if !LRPS2.iter().any(|name| name.eq_ignore_ascii_case(core)) {
        return BTreeMap::new();
    }
    let Some(serial) = crate::disc::ps2_serial(rom) else { return BTreeMap::new() };
    let known = LRPS2_GAMES.iter().find(|g| g.serial == serial);
    known.into_iter().flat_map(|g| g.options).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The core options RetroArch has for the core called `core` (its name without the version), by
/// key: what the game is known to need, then the core's, then the ones for `rom`'s folder, then
/// for `rom` itself, each over the last, as RetroArch takes them. With global core options on, there's one file for every core instead.
pub fn retroarch_core_options(core: &str, rom: &Path) -> BTreeMap<String, String> {
    let dir = Emulator::RetroArch.config_dir(false);
    core_options(&Ini::read(&dir.join("retroarch.cfg")), &dir, core, rom)
}

fn core_options(ini: &Ini, config_dir: &Path, core: &str, rom: &Path) -> BTreeMap<String, String> {
    let set = |key| ini.get("", key).filter(|v| !v.is_empty() && *v != "default");
    let configs = set("rgui_config_directory").map_or(config_dir.join("config"), expand_path).join(core);
    let mut files = Vec::new();
    if ini.get("", "global_core_options") == Some("true") {
        files.push(set("core_options_path").map_or(config_dir.join("retroarch-core-options.cfg"), expand_path));
    } else {
        files.push(configs.join(format!("{core}.opt")));
    }
    // Named by hand: a game called "Mr. Bones" isn't "Mr.opt".
    let opt = |name: &std::ffi::OsStr| configs.join(format!("{}.opt", name.to_string_lossy()));
    if let Some(folder) = rom.parent().and_then(|p| p.file_name()) {
        files.push(opt(folder));
    }
    if ini.get("", "game_specific_options") != Some("false")
        && let Some(name) = rom.file_stem()
    {
        files.push(opt(name));
    }
    let mut options = known_options(core, rom);
    for file in files {
        options.extend(Ini::read(&file).section("").map(|(k, v)| (k.to_string(), v.to_string())));
    }
    options
}

/// The remap RetroArch would play `rom` with in `core`, or none, so games play with the same
/// controls in romi.
pub fn retroarch_remap(core: &str, rom: &Path) -> Remap {
    let dir = Emulator::RetroArch.config_dir(false);
    remap(&Ini::read(&dir.join("retroarch.cfg")), &dir, core, rom)
}

/// The game's remap file, else its folder's, else the core's: only the first one found, as in
/// RetroArch. Remaps sorted by controller (`input_remap_sort_by_controller_enable`) aren't found.
fn remap(ini: &Ini, config_dir: &Path, core: &str, rom: &Path) -> Remap {
    // Both default to on, as RetroArch's do.
    if ini.get("", "auto_remaps_enable") == Some("false") || ini.get("", "input_remap_binds_enable") == Some("false") {
        return Remap::default();
    }
    let dir = ini
        .get("", "input_remapping_directory")
        .filter(|v| !v.is_empty() && *v != "default")
        .map_or(config_dir.join("config/remaps"), expand_path)
        .join(core);
    let names = [rom.file_stem(), rom.parent().and_then(|p| p.file_name()), Some(core.as_ref())];
    names
        .into_iter()
        .flatten()
        .find_map(|name| std::fs::read_to_string(dir.join(format!("{}.rmp", name.to_string_lossy()))).ok())
        .map(|text| Remap::parse(&Ini::parse(&text).section("").collect()))
        .unwrap_or_default()
}

fn save_dir(ini: &Ini, config_dir: &Path, core: &str, rom: &Path) -> Option<PathBuf> {
    if ini.get("", "savefiles_in_content_dir") == Some("true") {
        return None;
    }
    let mut dir = match ini.get("", "savefile_directory") {
        // RetroArch writes this line with its default when it first runs.
        None => config_dir.join("saves"),
        Some("" | "default") => return None,
        Some(dir) => expand_path(dir),
    };
    // Both default to on and off as RetroArch's do.
    if ini.get("", "sort_savefiles_enable") != Some("false") && !core.is_empty() {
        dir.push(core);
    }
    if ini.get("", "sort_savefiles_by_content_enable") == Some("true")
        && let Some(folder) = rom.parent().and_then(|p| p.file_name())
    {
        dir.push(folder);
    }
    Some(dir)
}

/// `key = value` lines under `[section]` headings (the ones before any heading are in ""), with
/// quotes taken off values. Enough for retroarch.cfg, Dolphin's and PCSX2's ini files and melonDS's
/// TOML. A key can appear more than once, as PCSX2 does for extra bindings.
#[derive(Default)]
struct Ini {
    entries: Vec<(String, String, String)>,
}

impl Ini {
    /// Empty if the file isn't there, which reads the same as an emulator's defaults.
    fn read(path: &Path) -> Ini {
        Ini::parse(&std::fs::read_to_string(path).unwrap_or_default())
    }

    fn parse(text: &str) -> Ini {
        let mut section = String::new();
        let mut entries = Vec::new();
        for line in text.lines().map(str::trim) {
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.trim().to_string();
            } else if let Some((key, value)) = line.split_once('=')
                && !line.starts_with(['#', ';'])
            {
                let value = value.trim();
                let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
                entries.push((section.clone(), key.trim().to_string(), value.to_string()));
            }
        }
        Ini { entries }
    }

    fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.section(section).find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    /// Set to something other than "".
    fn has(&self, section: &str, key: &str) -> bool {
        self.get(section, key).is_some_and(|v| !v.is_empty())
    }

    fn section<'a>(&'a self, section: &str) -> impl Iterator<Item = (&'a str, &'a str)> + use<'a> {
        let section = section.to_string();
        self.entries.iter().filter(move |(s, _, _)| *s == section).map(|(_, k, v)| (k.as_str(), v.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_folders_follow_retroarchs_settings() {
        let (config, rom) = (Path::new("/ra"), Path::new("/roms/neogeo/kof98.zip"));
        let at = |cfg: &str| save_dir(&Ini::parse(cfg), config, "FinalBurn Neo", rom);
        assert_eq!(at(""), Some(PathBuf::from("/ra/saves/FinalBurn Neo")));
        assert_eq!(at("savefile_directory = \"/s\"\nsort_savefiles_enable = \"false\""), Some(PathBuf::from("/s")));
        let by_content = "savefile_directory = \"/s\"\nsort_savefiles_by_content_enable = \"true\"";
        assert_eq!(at(by_content), Some(PathBuf::from("/s/FinalBurn Neo/neogeo")));
        assert_eq!(at("savefile_directory = \"default\""), None);
        assert_eq!(at("savefile_directory = \"/s\"\nsavefiles_in_content_dir = \"true\""), None);
    }

    #[test]
    fn ds_saves_are_where_melonds_keeps_them() {
        let dir = std::env::temp_dir().join(format!("romi-melonds-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rom = Path::new("/roms/nds/Pokemon - Platinum Version (USA).nds");
        let at = |toml: &str| {
            std::fs::write(dir.join("melonDS.toml"), toml).unwrap();
            melonds_save_dir(&dir, rom)
        };
        assert_eq!(at("[Instance0]\nSaveFilePath = \"\"\n"), Some(PathBuf::from("/roms/nds")));
        assert_eq!(at("[Instance0]\nSaveFilePath = \"/saves/ds\"\n"), Some(PathBuf::from("/saves/ds")));
        assert_eq!(at("[Instance0]\nSaveFilePath = \"saves\"\n"), None);
        std::fs::remove_file(dir.join("melonDS.toml")).unwrap();
        std::fs::write(dir.join("melonDS.ini"), "SaveFilePath=/old\n").unwrap();
        assert_eq!(melonds_save_dir(&dir, rom), Some(PathBuf::from("/old")));
        std::fs::remove_dir_all(&dir).unwrap();
        // Only melonDS's own core keeps saves as melonDS does.
        assert_eq!(shared_saves("retroarch -L {cores}/melonds_libretro.so {rom}", "melonDS", rom), None);
        assert_eq!(shared_saves("melonDS -f {rom}", "melonDS DS", rom), None);
    }

    #[test]
    fn gamecube_memory_cards_are_where_dolphin_keeps_them() {
        let dir = std::env::temp_dir().join(format!("romi-dolphin-test-{}", std::process::id()));
        let (config, data, legacy) = (dir.join("config"), dir.join("data"), dir.join("legacy"));
        std::fs::create_dir_all(&config).unwrap();
        let at = |ini: &str| {
            std::fs::write(config.join("Dolphin.ini"), ini).unwrap();
            dolphin_gc_dir(&config, &data, &legacy)
        };
        assert_eq!(at(""), Some(data.join("GC")), "a GCI folder by default");
        assert_eq!(at("[Core]\nSlotA = 8\nSlotB = 255\n"), Some(data.join("GC")));
        assert_eq!(at("[Core]\nSlotA = 1\n"), None, "a card in one file");
        assert_eq!(at("[Core]\nGCIFolderAPathOverride = /cards\n"), None);
        std::fs::create_dir_all(legacy.join("Config")).unwrap();
        assert_eq!(dolphin_gc_dir(&config, &data, &legacy), Some(legacy.join("GC")));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(shared_saves("dolphin-emu -b -e {rom}", "Dolphin", Path::new("/roms/gc/x.rvz")), None);
    }

    #[test]
    fn core_options_take_the_games_over_the_folders_over_the_cores() {
        let dir = std::env::temp_dir().join(format!("romi-core-options-test-{}", std::process::id()));
        let core = dir.join("config/Mupen64Plus-Next");
        std::fs::create_dir_all(&core).unwrap();
        let write = |name: &str, text: &str| std::fs::write(core.join(name), text).unwrap();
        write("Mupen64Plus-Next.opt", "cpu = \"interpreter\"\nres = \"640x480\"\naspect = \"4:3\"\n");
        write("n64.opt", "res = \"960x720\"\n");
        write("Mr. Bones.opt", "cpu = \"dynarec\"\n");
        let rom = Path::new("/roms/n64/Mr. Bones.z64");
        let options = |cfg: &str| core_options(&Ini::parse(cfg), &dir, "Mupen64Plus-Next", rom);
        let (all, no_games) = (options(""), options("game_specific_options = \"false\""));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(all["cpu"], "dynarec");
        assert_eq!(all["res"], "960x720");
        assert_eq!(all["aspect"], "4:3");
        assert_eq!(no_games["cpu"], "interpreter");
        assert_eq!(no_games["res"], "960x720");
    }

    #[test]
    fn a_game_known_to_need_an_option_gets_it_unless_an_option_file_says_otherwise() {
        let dir = std::env::temp_dir().join(format!("romi-known-options-test-{}", std::process::id()));
        let core = dir.join("config/ParaLLEl PS2");
        std::fs::create_dir_all(&core).unwrap();
        let rom = dir.join("ATV.iso");
        std::fs::write(&rom, crate::disc::tests::image("BOOT2 = cdrom0:\\SCUS_972.11;1\r\n")).unwrap();
        let other = dir.join("Other.iso");
        std::fs::write(&other, crate::disc::tests::image("BOOT2 = cdrom0:\\SLUS_000.01;1\r\n")).unwrap();
        let options = |core_name: &str, rom: &Path| core_options(&Ini::parse(""), &dir, core_name, rom);
        let got = options("ParaLLEl PS2", &rom);
        assert_eq!(got["pcsx2_hw_download_mode"], "Disable Readbacks");
        assert_eq!(options("LRPS2", &rom)["pcsx2_hw_download_mode"], "Disable Readbacks", "its old name too");
        assert!(options("ParaLLEl PS2", &other).is_empty(), "another game");
        assert!(options("Play!", &rom).is_empty(), "another core, whose options aren't these");
        // The core's own file, or the game's, beats it.
        std::fs::write(core.join("ParaLLEl PS2.opt"), "pcsx2_hw_download_mode = \"Hardware\"\n").unwrap();
        assert_eq!(options("ParaLLEl PS2", &rom)["pcsx2_hw_download_mode"], "Hardware");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ape_escape_is_given_a_dualshock() {
        let device = |core: &str, rom: &str| known_device(core, Path::new(rom));
        assert_eq!(device("Beetle PSX HW", "/roms/psx/Ape Escape (USA).chd"), Some(517));
        assert_eq!(device("Beetle PSX", "/roms/psx/Ape Escape (Europe).cue"), Some(517));
        assert_eq!(device("Beetle PSX HW", "/roms/psx/Crash Bandicoot (USA).chd"), None);
        assert_eq!(device("PCSX-ReARMed", "/roms/psx/Ape Escape (USA).chd"), None, "its ids aren't Beetle's");
    }

    #[test]
    fn remaps_take_the_games_else_the_folders_else_the_cores() {
        let dir = std::env::temp_dir().join(format!("romi-remap-test-{}", std::process::id()));
        let core = dir.join("config/remaps/Beetle PSX");
        std::fs::create_dir_all(&core).unwrap();
        let write = |name: &str, device: u32| {
            std::fs::write(core.join(name), format!("input_libretro_device_p1 = \"{device}\"\n")).unwrap();
        };
        write("Beetle PSX.rmp", 1);
        write("psx.rmp", 5);
        write("Ape Escape (USA).rmp", 517);
        let device = |cfg: &str, rom: &str| remap(&Ini::parse(cfg), &dir, "Beetle PSX", Path::new(rom)).devices[0];
        let found = [
            device("", "/roms/psx/Ape Escape (USA).chd"),
            device("", "/roms/psx/Crash Bandicoot (USA).chd"),
            device("", "/roms/ps1/Crash Bandicoot (USA).chd"),
            device("auto_remaps_enable = \"false\"", "/roms/psx/Ape Escape (USA).chd"),
            device("input_remap_binds_enable = \"false\"", "/roms/psx/Ape Escape (USA).chd"),
            device("input_remapping_directory = \"/nowhere\"", "/roms/psx/Ape Escape (USA).chd"),
        ];
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found, [Some(517), Some(5), Some(1), None, None, None]);
    }

    #[test]
    fn emulators_from_commands() {
        assert_eq!(Emulator::of("retroarch -L {cores}/melonds_libretro.so {rom}"), Some((Emulator::RetroArch, false)));
        assert_eq!(Emulator::of("~/Applications/pcsx2-v2.8.2.AppImage -batch {rom}"), Some((Emulator::Pcsx2, false)));
        assert_eq!(Emulator::of("flatpak run org.DolphinEmu.dolphin-emu -b -e {rom}"), Some((Emulator::Dolphin, true)));
        assert_eq!(Emulator::of("melonDS -f {rom}"), Some((Emulator::MelonDs, false)));
        assert_eq!(Emulator::of("mednafen {rom}"), None);
    }

    #[test]
    fn ini_sections_quotes_and_repeated_keys() {
        let ini = Ini::parse("a = \"1\"\n[Pad1]\nUp = SDL-0/DPadUp\nUp = SDL-0/-LeftY\n; Up = no\n[Other]\nUp = x\n");
        assert_eq!(ini.get("", "a"), Some("1"));
        assert_eq!(ini.section("Pad1").map(|(_, v)| v).collect::<Vec<_>>(), ["SDL-0/DPadUp", "SDL-0/-LeftY"]);
        assert!(!ini.has("Pad1", "Down"));
    }

    #[test]
    fn achievement_problems() {
        let dir = std::env::temp_dir().join(format!("romi-emucfg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("retroarch.cfg");
        let check = |text: &str| {
            std::fs::write(&cfg, text).unwrap();
            achievements(Emulator::RetroArch, &dir, "Me").unwrap()
        };
        let off = check("cheevos_enable = \"false\"\ncheevos_username = \"\"\ncheevos_token = \"\"\n");
        assert!(!off.ok && off.text.starts_with("RetroAchievements is off and not logged in"));
        let other = check("cheevos_enable = \"true\"\ncheevos_username = \"you\"\ncheevos_token = \"t\"\n");
        assert!(!other.ok && other.text.contains("as you, but romi shows Me's"));
        let fine = check("cheevos_enable = \"true\"\ncheevos_username = \"me\"\ncheevos_token = \"t\"\n");
        assert_eq!(fine, Note { ok: true, text: "RetroAchievements is on, logged in as me".into() });
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
