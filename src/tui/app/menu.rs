//! The menus and what they do to a game: favorites, tags, hiding, its cover and its emulator.

use super::input::Op;
use super::{App, Level};
use crate::config::{self, Emulator};
use crate::filter::{self};
use crate::state::State;
use std::path::{Path, PathBuf};

/// An entry in a menu: the main one (Start), or the selected game's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    /// Opens the selected game's menu.
    Game,
    Search,
    Filter,
    Sort,
    Random,
    Details,
    Grid,
    Stats,
    Achievements,
    ShowHidden,
    /// Opens the options panel.
    Options,
    /// Opens the players screen, with more than one controller connected.
    Players,
    Reload,
    Quit,
    Favorite,
    Hide,
    Emulator,
    /// Plays the game inside romi, in the middle panel.
    PlayHere,
    /// Starts the game in its emulator, when A plays it in romi instead.
    PlayOutside,
    /// Opens the controls screen on the game, which plays in romi.
    Controls,
    /// Opens the game's tags.
    Tags,
    /// Opens the achievements screen on the game.
    GameAchievements,
    /// One of your tags (by its place in name order), ticked when the game has it.
    Tag(usize),
    NewTag,
    /// Asks for an image to use as the game's cover.
    AddArt,
    /// Takes away the cover added with `AddArt`.
    RemoveArt,
}

/// An open menu. Everything that would otherwise need a keyboard is in one.
pub struct Menu {
    /// Whose menu: the selected game's, or None for the main menu.
    pub game: Option<usize>,
    pub selected: usize,
    /// The game's tags rather than its menu.
    pub tags: bool,
}

/// The prompt for an image to use as a game's cover. A terminal pastes the path of a file dropped
/// on it, and one that can't say it's a paste types it, so the text can be edited too.
pub struct ArtPrompt {
    /// Kept by key, since the config can be read again, and the games with it, while this is open.
    key: String,
    pub name: String,
    pub text: String,
    /// Why the last try didn't work.
    pub error: Option<String>,
}

impl App {
    pub(super) fn toggle_favorite(&mut self, g: usize) {
        let key = self.lib.games[g].key.clone();
        let mut added = false;
        self.state = State::update(|s| added = s.toggle_favorite(&key));
        let name = self.lib.games[g].name.clone();
        self.set_status(Level::Info, if added { format!("★ {name}") } else { format!("Unfavorited {name}") });
        if self.search.is_none() {
            self.rebuild_keeping_selection();
        }
    }

    /// Tags or untags a game. Tags live in state.json, and each shows as a collection.
    fn toggle_tag(&mut self, g: usize, tag: &str) {
        let key = self.lib.games[g].key.clone();
        let mut added = false;
        self.state = State::update(|s| added = s.toggle_tag(&key, tag));
        self.filter_options = filter::Options::from(&self.lib, &self.state);
        let name = &self.lib.games[g].name;
        self.set_status(Level::Info, if added { format!("{name}: #{tag}") } else { format!("{name}: no #{tag}") });
        if self.search.is_none() {
            self.rebuild_keeping_selection();
        }
    }

    /// Hides a game (or brings it back), saved in overrides.jsonc. The cursor stays where it was,
    /// on whatever moved into the hidden game's place.
    fn toggle_hidden(&mut self, g: usize) {
        let (key, name, hide) = {
            let game = &self.lib.games[g];
            (game.key.clone(), game.name.clone(), !game.hidden)
        };
        if let Err(e) = crate::overrides::update(&self.cfg_path, &key, |ov| ov.hidden = hide) {
            self.set_status(Level::Error, format!("Couldn't save: {e:#}"));
            return;
        }
        self.lib.games[g].hidden = hide;
        self.cfg.overrides.entry(key).or_default().hidden = hide;
        if self.search.is_some() {
            self.update_search();
        } else {
            let (view, at) = (self.views.get(self.view).map(|v| v.id.clone()), self.game);
            self.rebuild_views();
            self.select(view.as_deref(), None);
            self.game = at.min(self.list().len().saturating_sub(1));
        }
        let text = if !hide {
            format!("Unhid {name}")
        } else if self.show_hidden {
            format!("Hid {name}")
        } else {
            format!("Hid {name} (Show hidden games in the menu brings it back)")
        };
        self.set_status(Level::Info, text);
    }

    /// The cover added from a game's menu, if it has one. Only that is romi's to delete: an
    /// `"art"` written by hand can point at any file of yours.
    fn custom_art(&self, g: usize) -> Option<PathBuf> {
        let game = &self.lib.games[g];
        let own = crate::art::custom_path(&self.cfg_path, &self.lib.system_of(g).id, &game.key);
        game.art.manual.as_ref().is_some_and(|p| *p == own).then_some(own)
    }

    /// Tries what's in the art prompt. It stays open, saying why, until that works.
    pub(super) fn submit_art(&mut self) {
        let Some(prompt) = &self.art_prompt else { return };
        if prompt.text.trim().is_empty() {
            return;
        }
        let (key, source) = (prompt.key.clone(), crate::art::dropped_path(&prompt.text));
        match self.add_art(&key, &source) {
            Ok(()) => self.art_prompt = None,
            Err(e) => {
                if let Some(prompt) = &mut self.art_prompt {
                    prompt.error = Some(format!("{e:#}"));
                }
            }
        }
    }

    /// Saves a copy of an image as a game's cover, which its override in overrides.jsonc points at.
    fn add_art(&mut self, key: &str, source: &Path) -> anyhow::Result<()> {
        use anyhow::Context;
        let g = self.lib.find(key).context("That game isn't in the library any more")?;
        let dest = crate::art::custom_path(&self.cfg_path, &self.lib.system_of(g).id, key);
        let edited_elsewhere = self.edited_elsewhere();
        let replacing = dest.exists();
        crate::art::save_custom(source, &dest)?;
        let art = config::tilde(&dest);
        if let Err(e) = crate::overrides::update(&self.cfg_path, key, |ov| ov.art = Some(art)) {
            if !replacing {
                let _ = std::fs::remove_file(&dest);
            }
            return Err(e.context("Couldn't save"));
        }
        if self.art_changed(key, edited_elsewhere) {
            // The main config's overrides come before the file romi's menus write.
            let used = self.lib.find(key).is_some_and(|g| self.lib.games[g].art.manual.as_ref() == Some(&dest));
            match used {
                true => self.set_status(Level::Info, "Art saved"),
                false => self.set_status(Level::Warn, "Art saved, but the \"art\" in your config comes first"),
            }
        }
        Ok(())
    }

    /// Takes away the cover added from a game's menu, so its other art shows again.
    fn remove_art(&mut self, g: usize) {
        let Some(copy) = self.custom_art(g) else { return };
        let key = self.lib.games[g].key.clone();
        let edited_elsewhere = self.edited_elsewhere();
        if let Err(e) = crate::overrides::update(&self.cfg_path, &key, |ov| ov.art = None) {
            self.set_status(Level::Error, format!("Couldn't save: {e:#}"));
            return;
        }
        let deleted = std::fs::remove_file(&copy).or_else(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(e),
        });
        if self.art_changed(&key, edited_elsewhere) {
            match deleted {
                Ok(()) => self.set_status(Level::Info, "Art removed"),
                Err(e) => {
                    let text = format!("Art removed, but {} is still there: {e}", config::tilde(&copy));
                    self.set_status(Level::Warn, text);
                }
            }
        }
    }

    /// Shows what a game's override now says its cover is, without waiting for the file watcher to
    /// reload everything. False if everything was read again anyway, or couldn't be, as
    /// `read_config_again` says in the status.
    fn art_changed(&mut self, key: &str, edited_elsewhere: bool) -> bool {
        if !self.read_config_again(edited_elsewhere) {
            return false;
        }
        if let Some(g) = self.lib.find(key) {
            let art = self.cfg.overrides.get(key).and_then(|ov| ov.art.as_deref());
            self.lib.games[g].set_art_override(art);
        }
        if let Some(covers) = &mut self.covers {
            covers.forget_cover(key);
        }
        true
    }

    /// The ways a game can run, default first, once its menu has looked them up.
    pub fn emulator_options(&self, g: usize) -> Vec<Emulator> {
        self.emulators.options(&self.lib, g)
    }

    pub(super) fn current_emulator(&self, g: usize, options: &[Emulator]) -> usize {
        crate::emulators::current(&self.lib.games[g], options)
    }

    /// Switches a game to the next way it can run, saved in overrides.jsonc.
    fn cycle_emulator(&mut self, g: usize) {
        let options = self.emulator_options(g);
        if options.len() < 2 {
            return;
        }
        let next = (self.current_emulator(g, &options) + 1) % options.len();
        let pick = &options[next];
        let key = self.lib.games[g].key.clone();
        let saved = crate::overrides::update(&self.cfg_path, &key, |ov| {
            let picked = next > 0;
            ov.emulator = picked.then(|| pick.name.clone());
            ov.command = picked.then(|| pick.command.clone());
            ov.mode = pick.mode.filter(|_| picked);
            ov.cwd = pick.cwd.clone().filter(|_| picked);
        });
        if let Err(e) = saved {
            self.set_status(Level::Error, format!("Couldn't save: {e:#}"));
            return;
        }
        // The same as loading the override would do: what it doesn't say stays the default's.
        let game = &mut self.lib.games[g];
        let default = game.default_run.clone();
        game.launch.command.clone_from(&pick.command);
        game.launch.mode = pick.mode.or(default.mode).unwrap_or_default();
        game.launch.cwd = pick.cwd.clone().or(default.cwd);
        game.emulator = (next > 0).then(|| pick.name.clone());
        let text = format!("{} runs with {}", game.name, pick.name);
        self.set_status(Level::Info, text);
    }

    /// What the open menu offers, in order.
    pub fn menu_items(&self) -> Vec<MenuItem> {
        use MenuItem::*;
        let menu = self.menu.as_ref();
        if menu.is_some_and(|m| m.tags) {
            return (0..self.state.tags.len()).map(Tag).chain([NewTag]).collect();
        }
        match menu.and_then(|m| m.game) {
            Some(g) => {
                let mut items = Vec::new();
                if self.embeddable(g).is_some() {
                    if !self.a_plays_here() {
                        items.push(PlayHere);
                    } else if self.can_launch(g) {
                        items.push(PlayOutside);
                    }
                    items.push(Controls);
                }
                items.extend([Favorite, Hide, Tags]);
                if self.emulator_options(g).len() > 1 {
                    items.push(Emulator);
                }
                if self.achievements.games.contains_key(&self.lib.games[g].key) {
                    items.push(GameAchievements);
                }
                items.push(AddArt);
                if self.custom_art(g).is_some() {
                    items.push(RemoveArt);
                }
                items
            }
            None => {
                let mut items = Vec::new();
                if self.selected_game().is_some() {
                    items.push(Game);
                }
                items.extend([Search, Filter, Sort, Random, Grid, Details, Stats]);
                if !self.achievements.games.is_empty() {
                    items.push(Achievements);
                }
                if self.lib.games.iter().any(|g| g.hidden) {
                    items.push(ShowHidden);
                }
                if self.pads.len() > 1 {
                    items.push(Players);
                }
                items.extend([Options, Reload, Quit]);
                items
            }
        }
    }

    pub fn menu_label(&self, item: MenuItem) -> String {
        let game = self.menu.as_ref().and_then(|m| m.game).or(self.selected_game()).map(|g| &self.lib.games[g]);
        match item {
            MenuItem::Game => format!("{}…", game.map_or("Game", |g| g.name.as_str())),
            MenuItem::Search => "Search".into(),
            MenuItem::Filter => "Filter".into(),
            MenuItem::Sort => match self.sort() {
                Some(sort) => format!("Sort: {}", sort.label()),
                None => "Sort: newest first".into(),
            },
            MenuItem::Random => "Random game".into(),
            MenuItem::Stats => "Stats".into(),
            MenuItem::Achievements => "Achievements".into(),
            MenuItem::GameAchievements => "Achievements…".into(),
            MenuItem::Grid if self.state.grid => "List view".into(),
            MenuItem::Grid => "Cover grid".into(),
            MenuItem::Details if self.show_details => "Hide details".into(),
            MenuItem::Details => "Show details".into(),
            MenuItem::ShowHidden if self.show_hidden => "Hide hidden games".into(),
            MenuItem::ShowHidden => {
                format!("Show hidden games ({})", self.lib.games.iter().filter(|g| g.hidden).count())
            }
            MenuItem::Options => "Options…".into(),
            MenuItem::Players => "Players…".into(),
            MenuItem::Reload => "Reload config".into(),
            MenuItem::PlayHere => "Play in romi".into(),
            MenuItem::PlayOutside => match self.menu.as_ref().and_then(|m| m.game) {
                Some(g) => format!("Play with {}", self.runs_with_name(g)),
                None => "Play".into(),
            },
            MenuItem::Controls => "Controls…".into(),
            MenuItem::Quit => "Quit romi".into(),
            MenuItem::Favorite if game.is_some_and(|g| self.state.is_favorite(&g.key)) => "Unfavorite".into(),
            MenuItem::Favorite => "Favorite".into(),
            MenuItem::Hide if game.is_some_and(|g| g.hidden) => "Unhide".into(),
            MenuItem::Hide => "Hide".into(),
            MenuItem::Tags => match game.map(|g| self.state.tags_of(&g.key)) {
                Some(tags) if !tags.is_empty() => format!("Tags: {}", tags.join(", ")),
                _ => "Tags…".into(),
            },
            MenuItem::Tag(i) => {
                let tag = self.state.tags.keys().nth(i).map_or("", String::as_str);
                let on = game.is_some_and(|g| self.state.has_tag(&g.key, tag));
                format!("{} {tag}", if on { "✓" } else { " " })
            }
            MenuItem::NewTag => match &self.tag_name {
                Some(name) => format!("+ {name}█"),
                None => "+ New tag…".into(),
            },
            MenuItem::Emulator => {
                let g = self.menu.as_ref().and_then(|m| m.game);
                let options = g.map(|g| self.emulator_options(g)).unwrap_or_default();
                let at = g.map_or(0, |g| self.current_emulator(g, &options));
                format!("Emulator: {}", options.get(at).map_or("default", |e| e.name.as_str()))
            }
            MenuItem::AddArt => "Add art…".into(),
            MenuItem::RemoveArt => "Remove custom art".into(),
        }
    }

    pub(super) fn menu_op(&mut self, op: Op) {
        let items = self.menu_items();
        let Some(menu) = &mut self.menu else { return };
        let n = items.len().max(1);
        match op {
            Op::Up => menu.selected = (menu.selected + n - 1) % n,
            Op::Down => menu.selected = (menu.selected + 1) % n,
            // From the tags, back to the game's menu.
            Op::Back if menu.tags => {
                menu.tags = false;
                let at = self.menu_items().iter().position(|&i| i == MenuItem::Tags).unwrap_or(0);
                if let Some(menu) = &mut self.menu {
                    menu.selected = at;
                }
            }
            Op::Back | Op::Menu => self.menu = None,
            Op::Confirm => {
                let (game, selected, tags) = (menu.game, menu.selected, menu.tags);
                self.menu = None;
                let Some(&item) = items.get(selected) else { return };
                match item {
                    MenuItem::Game => self.apply(Op::GameMenu),
                    MenuItem::PlayHere => {
                        if let Some(g) = game {
                            self.play_here(g);
                        }
                    }
                    MenuItem::PlayOutside => {
                        if let Some(g) = game {
                            self.launch(g);
                        }
                    }
                    MenuItem::Controls => {
                        if let Some((g, (core, rom))) = game.and_then(|g| Some((g, self.embeddable(g)?))) {
                            let core = crate::emu::libretro::info(&core).map(|info| info.name);
                            self.controls = Some(crate::tui::controls::Screen::new(&self.lib, g, core, rom));
                        }
                    }
                    MenuItem::Search => self.apply(Op::Search),
                    MenuItem::Filter => self.apply(Op::Filter),
                    // Stays open, so A again tries the next order.
                    MenuItem::Sort => {
                        self.cycle_sort();
                        self.menu = Some(Menu { game, selected, tags: false });
                    }
                    MenuItem::Random => self.apply(Op::Random),
                    MenuItem::Grid => self.apply(Op::ToggleGrid),
                    MenuItem::Stats => self.apply(Op::Stats),
                    MenuItem::Achievements => self.apply(Op::Achievements),
                    MenuItem::GameAchievements => {
                        if let Some(g) = game {
                            self.cheevos = Some(crate::tui::cheevos::Screen::for_game(
                                &self.cfg,
                                &self.achievements,
                                &self.lib,
                                g,
                            ));
                        }
                    }
                    MenuItem::Details => self.apply(Op::ToggleDetails),
                    MenuItem::ShowHidden => {
                        self.show_hidden = !self.show_hidden;
                        self.rebuild_keeping_selection();
                    }
                    MenuItem::Options => self.options = Some(0),
                    MenuItem::Players => self.players = Some(crate::tui::players::Screen::default()),
                    MenuItem::Reload => self.reload(),
                    MenuItem::Quit => self.quit = true,
                    MenuItem::Favorite => {
                        if let Some(g) = game {
                            self.toggle_favorite(g);
                        }
                    }
                    MenuItem::Hide => {
                        if let Some(g) = game {
                            self.toggle_hidden(g);
                        }
                    }
                    MenuItem::Tags => self.menu = Some(Menu { game, selected: 0, tags: true }),
                    MenuItem::Tag(i) => {
                        if let (Some(g), Some(tag)) = (game, self.state.tags.keys().nth(i).cloned()) {
                            self.toggle_tag(g, &tag);
                            // Untagging the last game removes the tag, and the row with it.
                            let n = self.state.tags.len() + 1;
                            self.menu = Some(Menu { game, selected: selected.min(n - 1), tags });
                        }
                    }
                    MenuItem::NewTag => {
                        self.menu = Some(Menu { game, selected, tags });
                        self.tag_name = Some(String::new());
                        self.osk = Some((0, 0));
                    }
                    MenuItem::AddArt => {
                        if let Some(g) = game {
                            let game = &self.lib.games[g];
                            self.art_prompt = Some(ArtPrompt {
                                key: game.key.clone(),
                                name: game.name.clone(),
                                text: String::new(),
                                error: None,
                            });
                        }
                    }
                    MenuItem::RemoveArt => {
                        if let Some(g) = game {
                            self.remove_art(g);
                        }
                    }
                    // Stays open, like Sort.
                    MenuItem::Emulator => {
                        if let Some(g) = game {
                            self.cycle_emulator(g);
                        }
                        self.menu = Some(Menu { game, selected, tags: false });
                        // An emulator with a libretro core adds Play in romi above, or takes it away.
                        if let Some(at) = self.menu_items().iter().position(|&i| i == MenuItem::Emulator)
                            && let Some(menu) = &mut self.menu
                        {
                            menu.selected = at;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Tags the menu's game with the name typed (an existing tag if it matches one in any case),
    /// and leaves the cursor on it.
    pub(super) fn save_tag_name(&mut self) {
        let (Some(name), Some(g)) = (self.tag_name.take(), self.menu.as_ref().and_then(|m| m.game)) else { return };
        self.osk = None;
        let tag = self.state.tag_named(&name);
        if tag.is_empty() {
            return;
        }
        if !self.state.has_tag(&self.lib.games[g].key, &tag) {
            self.toggle_tag(g, &tag);
        }
        if let Some(menu) = &mut self.menu {
            menu.selected = self.state.tags.keys().position(|t| *t == tag).unwrap_or(0);
        }
    }

    pub(super) fn cancel_tag_name(&mut self) {
        self.tag_name = None;
        self.osk = None;
    }
}
