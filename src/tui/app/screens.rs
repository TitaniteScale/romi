//! What the keys do on the achievements, options, controls and players screens.

use super::input::Op;
use super::{App, Level, Watched, stamps};
use crate::config::{self};
use crate::state::State;

impl App {
    /// The achievements screen: the list of games, or one game's achievements.
    pub(super) fn cheevos_op(&mut self, op: Op) {
        let page = self.page.max(1) as isize;
        let Some(screen) = &mut self.cheevos else { return };
        if let Some(open) = &mut screen.open {
            let n = open.cheevos.as_ref().map_or(0, Vec::len) as isize;
            let scroll = |top: usize, by: isize| (top as isize + by).clamp(0, (n - 1).max(0)) as usize;
            match op {
                Op::Up => open.top = scroll(open.top, -1),
                Op::Down => open.top = scroll(open.top, 1),
                Op::PageUp => open.top = scroll(open.top, -page / 2),
                Op::PageDown => open.top = scroll(open.top, page / 2),
                Op::Back | Op::Menu | Op::Confirm => {
                    screen.open = None;
                    if screen.just_one || matches!(op, Op::Menu) {
                        self.cheevos = None;
                    }
                }
                _ => {}
            }
            return;
        }
        let rows = screen.rows(&self.lib, &self.achievements, self.show_hidden);
        let n = rows.len() as isize;
        let move_to = |at: usize, by: isize| (at as isize + by).clamp(0, (n - 1).max(0)) as usize;
        match op {
            Op::Up => screen.selected = move_to(screen.selected, -1),
            Op::Down => screen.selected = move_to(screen.selected, 1),
            Op::PageUp => screen.selected = move_to(screen.selected, -page),
            Op::PageDown => screen.selected = move_to(screen.selected, page),
            Op::Left | Op::PrevSystem => screen.step_system(&self.lib, &self.achievements, false),
            Op::Right | Op::NextSystem => screen.step_system(&self.lib, &self.achievements, true),
            Op::Random | Op::Favorite => {
                screen.status = screen.status.step(matches!(op, Op::Random));
                screen.selected = 0;
            }
            Op::Search => {
                screen.typing = true;
                self.osk = Some((0, 0));
            }
            Op::Confirm => {
                if let Some(&g) = rows.get(screen.selected) {
                    screen.open_game(&self.cfg, &self.achievements, &self.lib, g);
                }
            }
            Op::Back | Op::Menu => self.cheevos = None,
            _ => {}
        }
    }

    /// The filter panel: up/down picks a row, left/right (or A) changes it, and the list updates
    /// behind the panel as you go.
    /// The options panel: up/down picks a setting, left/right (or A) changes it, saving it into
    /// the config straight away.
    pub(super) fn options_op(&mut self, op: Op) {
        use crate::tui::options::SETTINGS;
        let Some(row) = self.options else { return };
        let n = SETTINGS.len();
        match op {
            Op::Up => self.options = Some((row + n - 1) % n),
            Op::Down => self.options = Some((row + 1) % n),
            Op::Back | Op::Menu => self.options = None,
            Op::Left => self.change_setting(SETTINGS[row], false),
            Op::Right | Op::Confirm => self.change_setting(SETTINGS[row], true),
            _ => {}
        }
    }

    /// Steps a setting to its next value and saves it into the config, changing only that value.
    fn change_setting(&mut self, setting: crate::tui::options::Setting, forward: bool) {
        let value = setting.step(&self.cfg, forward);
        if self.save_setting(setting.path(), &value) {
            let shown = setting.shown(&self.cfg);
            self.set_status(Level::Info, format!("{}: {shown}", setting.label()));
        }
    }

    /// Saves one setting into the config, changing only that value, and reads the config again.
    /// False, with the error in the status, if it couldn't, or if everything was read again because
    /// the config had been changed by hand too.
    fn save_setting(&mut self, path: &[&str], value: &serde_json::Value) -> bool {
        let edited_elsewhere = self.edited_elsewhere();
        if let Err(e) = crate::config_edit::save(&self.cfg_path, path, value) {
            self.set_status(Level::Error, format!("{e:#}"));
            return false;
        }
        self.read_config_again(edited_elsewhere)
    }

    /// The config's files have changed since romi last read them.
    pub(super) fn edited_elsewhere(&self) -> bool {
        stamps(&self.watched.files) != self.watched.seen
    }

    /// Reads the config again after romi has saved a change to it, or everything, if it had been
    /// changed by hand too (`edited_elsewhere`), in which case it's false, as when it can't be read.
    pub(super) fn read_config_again(&mut self, edited_elsewhere: bool) -> bool {
        if edited_elsewhere {
            self.reload_as("Saved, and reloaded");
            return false;
        }
        match config::load(&self.cfg_path) {
            Ok(cfg) => {
                let art_changed = (cfg.art.show, cfg.art.screens) != (self.cfg.art.show, self.cfg.art.screens);
                if let Some(c) = self.covers.as_mut().filter(|_| art_changed) {
                    c.reset(&cfg);
                }
                self.cfg = cfg;
                self.apply_settings();
                self.watched = Watched::new(&self.cfg_path, &self.cfg);
                true
            }
            Err(e) => {
                self.set_status(Level::Error, format!("Config error: {e:#}"));
                false
            }
        }
    }

    /// Closes the players screen, saving the order if a controller was pressed.
    pub(super) fn close_players(&mut self) {
        let Some(screen) = self.players.take() else { return };
        if screen.pressed.is_empty() {
            return;
        }
        let order = screen.to_save(&self.pads, &self.state.players);
        self.state = State::update(|s| s.players = order);
        let shown = screen.order(&self.pads, &self.state.players);
        let shown: Vec<_> = shown.iter().enumerate().map(|(i, p)| format!("{} {p}", i + 1)).collect();
        self.set_status(Level::Info, format!("Players: {}", shown.join(", ")));
    }

    /// The controls screen: up and down pick a row, and A on a button waits for the one to press
    /// for it. Left and right (or A) on the first row switch between the game and its system.
    pub(super) fn controls_op(&mut self, op: Op) {
        use crate::tui::controls::Row;
        let Some(screen) = &mut self.controls else { return };
        // Waiting for a button, the stick moves nothing.
        if screen.capturing {
            return;
        }
        let n = screen.rows().len();
        match (op, screen.row()) {
            (Op::Up, _) => screen.selected = (screen.selected + n - 1) % n,
            (Op::Down, _) => screen.selected = (screen.selected + 1) % n,
            (Op::Back | Op::Menu, _) => self.controls = None,
            (Op::Left | Op::Right | Op::Confirm, Row::For) => screen.for_system = !screen.for_system,
            (Op::Confirm, Row::Button(_)) => screen.capturing = true,
            (Op::Confirm, Row::Reset) => {
                self.save_controls(crate::emu::remap::Controls::default());
            }
            _ => {}
        }
    }

    /// The pad's button `pressed` now sends the button the controls screen was waiting for.
    pub(super) fn press_for_controls(&mut self, pressed: &str) {
        let Some(screen) = &mut self.controls else { return };
        screen.capturing = false;
        if let Some(controls) = screen.press(&self.cfg, &self.lib, pressed) {
            self.save_controls(controls);
        }
    }

    /// Saves the controls screen's buttons for its game, in overrides.jsonc, or for its system, in
    /// the config.
    fn save_controls(&mut self, controls: crate::emu::remap::Controls) {
        let Some(screen) = &self.controls else { return };
        let g = screen.game;
        let (key, system) = (self.lib.games[g].key.clone(), self.lib.system_of(g).id.clone());
        if screen.for_system {
            let Ok(value) = serde_json::to_value(&controls) else { return };
            if self.save_setting(&["controls", &system], &value) {
                self.set_status(Level::Info, format!("Saved the controls for {}", self.lib.system_of(g).name));
            }
            return;
        }
        let edited_elsewhere = self.edited_elsewhere();
        if let Err(e) = crate::overrides::update(&self.cfg_path, &key, |ov| ov.controls = controls.clone()) {
            return self.set_status(Level::Error, format!("Couldn't save: {e:#}"));
        }
        if self.read_config_again(edited_elsewhere) {
            self.lib.games[g].controls = controls;
            self.set_status(Level::Info, format!("Saved the controls for {}", self.lib.games[g].name));
        }
    }
}
