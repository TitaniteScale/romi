//! Starting games, in romi or in their emulator, and coming back from them.

use super::{App, Level, Running, TerminalJob, log_path};
use crate::config::{self, LaunchMode};
use crate::hooks::Hooks;
use crate::session::Session;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// After a game exits, ignore input briefly so the button used to quit doesn't launch something.
const INPUT_COOLDOWN: Duration = Duration::from_millis(600);

impl App {
    /// The core and ROM to play a game inside romi with, if it runs through a libretro core.
    pub(super) fn embeddable(&self, g: usize) -> Option<(PathBuf, PathBuf)> {
        crate::emulators::playable(&self.lib.games[g], &self.emulator_options(g))
    }

    /// Whether the program a game's command runs is installed, so it can start outside romi.
    pub(super) fn can_launch(&self, g: usize) -> bool {
        self.lib.games[g].launch.prepare().and_then(|p| p.check_program()).is_ok()
    }

    /// Whether A plays games inside romi, where they can play there. Not in a terminal they don't
    /// show in, though the game's menu can still try.
    pub(super) fn a_plays_here(&self) -> bool {
        self.cfg.play_in_romi && crate::emu::kitty::terminal().shows_games
    }

    /// Plays a game as A does: inside romi when it can, or else in its emulator.
    pub(super) fn play(&mut self, g: usize) {
        if self.cfg.play_in_romi {
            // A game that runs in another emulator may play in romi through one of its others.
            self.emulators.find(&self.cfg, &self.lib, g);
            if self.embeddable(g).is_some() {
                if self.a_plays_here() {
                    return self.play_here(g);
                }
                return self.launch_instead(g);
            }
        }
        self.launch(g);
    }

    /// Starts a game that could play in romi in its emulator, because of the terminal, saying why
    /// the first time, or if it can't start.
    fn launch_instead(&mut self, g: usize) {
        if let Err(e) = self.lib.games[g].launch.prepare().and_then(|p| p.check_program()) {
            return self.set_status(Level::Error, format!("Games only show in Ghostty or kitty, and {e:#}"));
        }
        self.launch(g);
        if std::mem::replace(&mut self.told_about_terminal, true) {
            return;
        }
        let why =
            format!("{} can't show games in romi, so A starts their emulators", crate::emu::kitty::terminal().name);
        match self.running {
            Some(_) => self.after_game = Some(why),
            None => self.set_status(Level::Warn, why),
        }
    }

    /// What a game runs with outside romi, by the name of its emulator: the one picked for it in
    /// its menu, or the program its command runs.
    pub(super) fn runs_with_name(&self, g: usize) -> String {
        let options = self.emulator_options(g);
        match options.get(self.current_emulator(g, &options)) {
            Some(e) => e.name.clone(),
            None => crate::emulators::runs_label(&self.lib.games[g].launch.command),
        }
    }

    pub(super) fn play_here(&mut self, g: usize) {
        let Some((core, rom)) = self.embeddable(g) else { return };
        if let Err(e) = self.try_core(&core, &rom) {
            return self.set_status(Level::Error, format!("{}: {e}", self.lib.games[g].name));
        }
        let volume = self.cfg.volume_gain();
        let launch = self.launch_animation(g, &rom);
        let cheevos = self.cheevos_setup(g);
        let look = crate::emu::embedded::Appearance {
            corners: self.cfg.corners(),
            filter: self.cfg.screen_filter,
            crop: self.cfg.crop_borders,
            badges: self.cfg.retroachievements.badges,
        };
        let controls = crate::emu::runner::controls_for(&self.cfg, &self.lib, g);
        let core_options = crate::emu::runner::core_options_for(&self.cfg, &self.lib, g);
        let runs_with = Some(self.lib.games[g].launch.command.as_str());
        let states = crate::emu::savestate::Files::new(&core, &rom);
        let slot = self.state.state_slots.get(&self.lib.games[g].key).copied().unwrap_or(0);
        let start = crate::emu::embedded::Start {
            game: g,
            volume,
            launch,
            look,
            cheevos,
            pixel_mouse: self.pixel_mouse,
            states,
            slot,
        };
        let started =
            crate::emu::runner::load(&self.cfg, &core, &rom, runs_with, &controls, &core_options, self.shown_height())
                .and_then(|core| crate::emu::embedded::Embedded::start(core, start));
        match started {
            Ok(e) => {
                self.embedded = Some(e);
                // Recorded as any other game is. Its process is this one, which is how `romi guide`
                // knows to leave the Guide button alone while it plays.
                let hooks = Hooks::for_game(&self.cfg, &self.lib, g);
                if let Some(problem) = hooks.launch() {
                    self.set_status(Level::Warn, problem);
                }
                let (session, state) = Session::begin(&self.lib, g, std::process::id(), hooks);
                self.state = state;
                self.embedded_session = Some(session);
            }
            Err(e) => self.set_status(Level::Error, format!("{}: {e:#}", self.lib.games[g].name)),
        }
    }

    /// About how many screen pixels tall a game played here will be: the middle panel's inside,
    /// less the hint under it, or as tall as fits its width for a 4:3 picture, the shape of the
    /// systems that draw with the GPU.
    fn shown_height(&self) -> Option<u32> {
        let size = ratatui::crossterm::terminal::window_size().ok()?;
        let (cell_w, cell_h) = crate::emu::kitty::cell_size(&size);
        let width = u32::from(self.middle.width.saturating_sub(2)) * cell_w;
        let height = u32::from(self.middle.height.saturating_sub(3)) * cell_h;
        Some(height.min(width * 3 / 4)).filter(|&h| h > 0)
    }

    /// What a game played here needs for its achievements: the RetroAchievements login and the
    /// ROM's hash. None without a login, or for a system RetroAchievements doesn't hash.
    fn cheevos_setup(&self, g: usize) -> Option<crate::emu::cheevos::Setup> {
        let login = crate::emu::cheevos::saved_login()?;
        let (console, hash) = crate::ra::game_hash(&self.lib, g)?;
        Some(crate::emu::cheevos::Setup { login, console, hash })
    }

    /// The launch animation for a game played here, if it's on: its system's console, or a TV.
    /// The ROM's file name seeds it, so a game always starts the same way.
    fn launch_animation(&self, g: usize, rom: &Path) -> Option<crate::emu::embedded::Launch> {
        use crate::emu::wireframe;
        if !self.cfg.launch_animation.enabled {
            return None;
        }
        let model = wireframe::Model::load(wireframe::for_system(&self.lib.system_of(g).id)).ok()?;
        let seed = wireframe::fnv1a(&rom.file_name()?.to_string_lossy());
        Some(crate::emu::embedded::Launch { model, seed, beam: self.look.beam(seed) })
    }

    /// Starts the core in a separate romi for a frame, the first time it's played here. A core
    /// that wants what romi can't give it can crash while loading (Flycast did, before romi could
    /// give it OpenGL), and in this process that would take the TUI down with it.
    fn try_core(&mut self, core: &Path, rom: &Path) -> Result<(), String> {
        if self.cores_tried.contains(core) {
            return Ok(());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let out = std::process::Command::new(exe)
            .arg("run")
            .arg(rom)
            .arg("--core")
            .arg(core)
            .args(["--frames", "1"])
            .env("ROMI_CONFIG", &self.cfg_path)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| e.to_string())?;
        match out.status.code() {
            Some(0) => {
                self.cores_tried.insert(core.to_path_buf());
                Ok(())
            }
            Some(_) => {
                let err = String::from_utf8_lossy(&out.stderr);
                let last = err.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("the core didn't start");
                Err(last.trim_start_matches("romi: ").to_string())
            }
            None => Err("the core crashed while starting (ROMI_CORE_LOG=file says what it was doing)".into()),
        }
    }

    /// Stops the game in the middle panel. Input waits a moment, so the buttons that stopped it
    /// don't also do something in the list.
    pub fn stop_embedded(&mut self) {
        if let Some(mut embedded) = self.embedded.take() {
            let g = embedded.game;
            if let Some(session) = self.embedded_session.take() {
                let (_, state, _hooks) = session.end(None);
                self.state = state;
            }
            match embedded.write_timings() {
                Some(Ok(path)) => self.set_status(Level::Info, format!("Frame timings in {}", config::tilde(&path))),
                Some(Err(e)) => self.set_status(Level::Warn, format!("{e:#}")),
                None => {}
            }
            if let Some(e) = embedded.stop() {
                self.set_status(Level::Error, format!("Not saved: {e}"));
            }
            // For the controls screen, which can't ask the core.
            let names = crate::emu::libretro::button_names();
            crate::tui::controls::remember(&self.lib.games[g].key, &self.lib.system_of(g).id, names);
            self.rebuild_keeping_selection();
        }
        self.cooldown_until = Some(Instant::now() + INPUT_COOLDOWN);
    }

    /// Feeds the game in the middle panel: controller input in, the newest frame out.
    pub fn pump_embedded(&mut self) {
        let Some(embedded) = &mut self.embedded else { return };
        if !embedded.read_pads() {
            return self.stop_embedded();
        }
        if let Err(e) = embedded.pump() {
            self.stop_embedded();
            return self.set_status(Level::Error, format!("Stopped: {e:#}"));
        }
        let (sound_error, save_error) = (embedded.sound_error(), embedded.save_error());
        let (state_news, slot_moved, game) = (embedded.state_news(), embedded.take_slot_moved(), embedded.game);
        if let Some(say) = embedded.cheevo_status() {
            self.set_status(Level::Warn, say);
        }
        if let Some(e) = sound_error {
            self.set_status(Level::Warn, format!("No sound: {e}"));
        }
        if let Some(e) = save_error {
            self.set_status(Level::Error, format!("Not saved: {e}"));
        }
        for news in state_news {
            match news {
                Ok(say) => self.set_status(Level::Info, say),
                Err(say) => self.set_status(Level::Error, say),
            }
        }
        if let Some(slot) = slot_moved {
            let key = self.lib.games[game].key.clone();
            self.state = crate::state::State::update(|s| {
                if slot == 0 {
                    s.state_slots.remove(&key);
                } else {
                    s.state_slots.insert(key, slot);
                }
            });
        }
    }

    pub(super) fn launch(&mut self, g: usize) {
        let game = &self.lib.games[g];
        let name = game.name.clone();
        let prepared = match game.launch.prepare() {
            Ok(p) => p,
            Err(e) => {
                self.set_status(Level::Error, format!("{name}: {e:#}"));
                return;
            }
        };
        let result = match game.launch.mode {
            LaunchMode::Wait => {
                let hooks = Hooks::for_game(&self.cfg, &self.lib, g);
                let problem = hooks.launch();
                let started = prepared.spawn_logged(&log_path(), true).map(|child| {
                    let (session, state) = Session::begin(&self.lib, g, child.id(), hooks);
                    self.state = state;
                    self.running = Some(Running { game: g, child, started: Instant::now(), session });
                });
                if let Some(problem) = problem {
                    self.set_status(Level::Warn, problem);
                }
                started
            }
            // A copy of romi plays it in the background and records the session, so play time
            // counts even if this one quits first.
            LaunchMode::Detach => prepared
                .check_program()
                .and_then(|()| crate::session::play_in_background(&self.cfg_path, &game.key))
                .map(|mut watcher| {
                    std::thread::spawn(move || watcher.wait());
                    self.set_status(Level::Info, format!("Started {name}"));
                }),
            // The main loop releases the terminal, then starts it (see `run_in_terminal`).
            LaunchMode::Terminal => {
                self.terminal_job = Some(TerminalJob { game: g, prepared });
                Ok(())
            }
        };
        if let Err(e) = result {
            self.set_status(Level::Error, format!("{e:#}"));
        }
    }

    /// Back to the list after a game exits (wait or terminal mode). Its session has already
    /// been recorded.
    pub fn finish(&mut self, g: usize, played: Duration, code: Option<Option<i32>>) {
        let name = self.lib.games[g].name.clone();
        // Whatever window had focus is gone; assume we're back in front.
        self.term_focused = true;
        self.cooldown_until = Some(Instant::now() + INPUT_COOLDOWN);
        match code {
            Some(Some(0)) => {
                self.set_status(Level::Info, format!("Played {name} for {}", crate::text::duration(played.as_secs())))
            }
            Some(Some(c)) => self
                .set_status(Level::Error, format!("{name} exited with code {c} (log: {})", config::tilde(&log_path()))),
            Some(None) => self.set_status(Level::Warn, format!("{name} was killed by a signal")),
            None => self.set_status(Level::Error, format!("Lost track of {name}")),
        }
        if let Some(why) = self.after_game.take() {
            self.set_status(Level::Warn, why);
        }
        self.rebuild_keeping_selection();
    }
}
