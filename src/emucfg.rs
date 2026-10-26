//! What `romi check` can tell from emulators' own settings files: whether RetroAchievements is on
//! and logged in, and whether a controller is bound. romi only ever reads these files.

use crate::config::{Config, expand_path};
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
