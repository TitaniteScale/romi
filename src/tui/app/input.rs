//! Input: keys, the mouse and controller buttons all funnel into the same `Op`s, which `apply` carries out.

use super::menu::Menu;
use super::{App, PadState, Pane};
use crate::gamepad::{PadAction, PadEvent};
use crate::state::State;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::time::Instant;

pub(super) enum Op {
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Left,
    Right,
    Confirm,
    Back,
    NextSystem,
    PrevSystem,
    NextLetter,
    PrevLetter,
    Favorite,
    Random,
    ToggleDetails,
    Menu,
    Search,
    Filter,
    Sort,
    GameMenu,
    ToggleGrid,
    Stats,
    Achievements,
}

fn random_below(n: usize) -> usize {
    // No need for a crate: the clock's nanoseconds, scrambled a little, are plenty random here.
    let mut x =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x % n as u64) as usize
}

impl App {
    fn input_blocked(&self) -> bool {
        self.running.is_some() || self.cooldown_until.is_some_and(|t| Instant::now() < t)
    }

    pub fn on_term_event(&mut self, ev: Event) {
        // A game in the middle panel gets every key, releases too, and the mouse.
        if let (Some(embedded), Event::Key(k)) = (&mut self.embedded, &ev) {
            if !embedded.on_key(*k) {
                self.stop_embedded();
            }
            return;
        }
        if let (Some(embedded), Event::Mouse(m)) = (&mut self.embedded, &ev) {
            return embedded.on_mouse(*m);
        }
        match ev {
            Event::FocusGained => {
                self.term_focused = true;
                self.reload_state();
            }
            Event::FocusLost => self.term_focused = false,
            Event::Key(k) if k.kind != KeyEventKind::Release && !self.input_blocked() => {
                self.on_key(k);
            }
            Event::Paste(text) if self.embedded.is_none() && !self.input_blocked() => self.on_paste(&text),
            _ => {}
        }
    }

    /// A paste: the art prompt takes it as a dropped file, and a text field as if it was typed.
    /// Anywhere else it's ignored, rather than each letter being taken for a key's action.
    fn on_paste(&mut self, text: &str) {
        if let Some(prompt) = &mut self.art_prompt {
            prompt.text = crate::art::first_line(text).to_string();
            self.submit_art();
        } else if self.typing() {
            for c in text.chars().filter(|c| !c.is_control()) {
                self.edit_query(Some(c));
            }
        }
    }

    /// Whether keys go to a text field `edit_query` knows, as in `on_key`.
    fn typing(&self) -> bool {
        if self.tag_name.is_some() {
            return true;
        }
        if let Some(screen) = &self.cheevos {
            return screen.typing;
        }
        let covered = self.stats.is_some()
            || self.players.is_some()
            || self.controls.is_some()
            || self.filter_panel.is_some()
            || self.options.is_some()
            || self.menu.is_some();
        self.search.is_some() && !covered
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if ctrl && k.code == KeyCode::Char('r') {
            self.reload();
            return;
        }
        if let Some(prompt) = &mut self.art_prompt {
            match k.code {
                KeyCode::Enter => self.submit_art(),
                KeyCode::Esc => self.art_prompt = None,
                KeyCode::Backspace => {
                    prompt.text.pop();
                    prompt.error = None;
                }
                KeyCode::Char(c) if !ctrl => {
                    prompt.text.push(c);
                    prompt.error = None;
                }
                _ => {}
            }
            return;
        }
        if let Some(name) = &mut self.tag_name {
            match k.code {
                KeyCode::Enter => self.save_tag_name(),
                KeyCode::Esc => self.cancel_tag_name(),
                KeyCode::Backspace => {
                    name.pop();
                }
                KeyCode::Char(c) if !ctrl => name.push(c),
                _ => {}
            }
            return;
        }
        if self.stats.is_some() {
            if matches!(k.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Backspace | KeyCode::Char('q' | 'S' | 'm')) {
                self.stats = None;
            }
            return;
        }
        if let Some(screen) = &mut self.cheevos {
            if screen.typing {
                match k.code {
                    KeyCode::Enter | KeyCode::Esc => {
                        screen.typing = false;
                        self.osk = None;
                    }
                    KeyCode::Backspace => {
                        screen.query.pop();
                        screen.selected = 0;
                    }
                    KeyCode::Char(c) if !ctrl => {
                        screen.query.push(c);
                        screen.selected = 0;
                    }
                    _ => {}
                }
                return;
            }
            let op = match k.code {
                KeyCode::Up | KeyCode::Char('k') => Op::Up,
                KeyCode::Down | KeyCode::Char('j') => Op::Down,
                KeyCode::PageUp => Op::PageUp,
                KeyCode::PageDown => Op::PageDown,
                KeyCode::Char('u') if ctrl => Op::PageUp,
                KeyCode::Char('d') if ctrl => Op::PageDown,
                KeyCode::Left | KeyCode::Char('h' | '[') => Op::PrevSystem,
                KeyCode::Right | KeyCode::Char('l' | ']') => Op::NextSystem,
                KeyCode::Tab => Op::Random,
                KeyCode::BackTab => Op::Favorite,
                KeyCode::Char('/') => Op::Search,
                KeyCode::Enter => Op::Confirm,
                KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q' | 'A' | 'm') => Op::Back,
                _ => return,
            };
            self.cheevos_op(op);
            return;
        }
        if self.players.is_some() {
            if matches!(k.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                self.close_players();
            }
            return;
        }
        if let Some(screen) = &mut self.controls {
            if screen.capturing {
                match (k.code, crate::emu::controls::key_name(k.code)) {
                    (KeyCode::Esc, _) => screen.capturing = false,
                    (_, Some(button)) => self.press_for_controls(button),
                    _ => {}
                }
                return;
            }
            let op = match k.code {
                KeyCode::Up | KeyCode::Char('k') => Op::Up,
                KeyCode::Down | KeyCode::Char('j') => Op::Down,
                KeyCode::Left | KeyCode::Char('h') => Op::Left,
                KeyCode::Right | KeyCode::Char('l') => Op::Right,
                KeyCode::Enter => Op::Confirm,
                KeyCode::Esc | KeyCode::Char('q') => Op::Back,
                _ => return,
            };
            self.apply(op);
            return;
        }
        if self.filter_panel.is_some() || self.options.is_some() {
            let op = match k.code {
                KeyCode::Up | KeyCode::Char('k') => Op::Up,
                KeyCode::Down | KeyCode::Char('j') => Op::Down,
                KeyCode::Left | KeyCode::Char('h') => Op::Left,
                KeyCode::Right | KeyCode::Char('l') => Op::Right,
                KeyCode::Enter => Op::Confirm,
                KeyCode::Esc | KeyCode::Char('q' | 'F') => Op::Back,
                _ => return,
            };
            self.apply(op);
            return;
        }
        if self.menu.is_some() {
            let op = match k.code {
                KeyCode::Up | KeyCode::Char('k') => Op::Up,
                KeyCode::Down | KeyCode::Char('j') => Op::Down,
                KeyCode::Enter => Op::Confirm,
                KeyCode::Esc | KeyCode::Char('q' | 'm' | 'o') => Op::Back,
                _ => return,
            };
            self.apply(op);
            return;
        }

        if self.search.is_some() {
            let op = match k.code {
                KeyCode::Up => Some(Op::Up),
                KeyCode::Down => Some(Op::Down),
                KeyCode::Char('p') if ctrl => Some(Op::Up),
                KeyCode::Char('n') if ctrl => Some(Op::Down),
                KeyCode::PageUp => Some(Op::PageUp),
                KeyCode::PageDown => Some(Op::PageDown),
                KeyCode::Enter => Some(Op::Confirm),
                KeyCode::Esc => Some(Op::Back),
                KeyCode::Backspace => {
                    let s = self.search.as_mut().unwrap();
                    if s.query.pop().is_none() {
                        self.end_search();
                    } else {
                        self.update_search();
                    }
                    None
                }
                KeyCode::Char(c) if !ctrl => {
                    self.search.as_mut().unwrap().query.push(c);
                    self.update_search();
                    None
                }
                _ => None,
            };
            if let Some(op) = op {
                self.apply(op);
            }
            return;
        }

        let op = match k.code {
            KeyCode::Char('q') => {
                self.quit = true;
                return;
            }
            KeyCode::Char('/') => {
                self.start_search();
                return;
            }
            KeyCode::Up | KeyCode::Char('k') => Op::Up,
            KeyCode::Down | KeyCode::Char('j') => Op::Down,
            KeyCode::Char('u') if ctrl => Op::PageUp,
            KeyCode::Char('d') if ctrl => Op::PageDown,
            KeyCode::PageUp => Op::PageUp,
            KeyCode::PageDown => Op::PageDown,
            KeyCode::Home | KeyCode::Char('g') => Op::Top,
            KeyCode::End | KeyCode::Char('G') => Op::Bottom,
            KeyCode::Left | KeyCode::Char('h') => Op::Left,
            KeyCode::Right | KeyCode::Char('l') => Op::Right,
            KeyCode::Enter => Op::Confirm,
            KeyCode::Esc | KeyCode::Backspace => Op::Back,
            KeyCode::Tab | KeyCode::Char(']') => Op::NextSystem,
            KeyCode::BackTab | KeyCode::Char('[') => Op::PrevSystem,
            KeyCode::Char('f') => Op::Favorite,
            KeyCode::Char('r') => Op::Random,
            KeyCode::Char('d') => Op::ToggleDetails,
            KeyCode::Char('m') => Op::Menu,
            KeyCode::Char('F') => Op::Filter,
            KeyCode::Char('s') => Op::Sort,
            KeyCode::Char('o') => Op::GameMenu,
            KeyCode::Char('v') => Op::ToggleGrid,
            KeyCode::Char('S') => Op::Stats,
            KeyCode::Char('A') => Op::Achievements,
            _ => return,
        };
        self.apply(op);
    }

    pub fn on_pad(&mut self, ev: PadEvent) {
        match ev {
            PadEvent::Connected(name) => self.pad = PadState::Connected(name),
            PadEvent::Disconnected => self.pad = PadState::Searching,
            PadEvent::Unavailable(e) => self.pad = PadState::Unavailable(e),
            PadEvent::Pads(pads) => self.pads = pads,
            PadEvent::Pressed { .. } | PadEvent::Action(_) => {
                // A game in the middle panel reads the controller itself.
                if self.embedded.is_some()
                    || self.input_blocked()
                    || (self.cfg.controller.require_focus && !self.term_focused)
                {
                    return;
                }
                if let PadEvent::Pressed { pad, action, .. } = &ev
                    && let Some(screen) = &mut self.players
                {
                    // Each controller's first press makes it the next player; then B is done.
                    if !screen.pressed.contains(pad) {
                        screen.pressed.push(pad.clone());
                    } else if *action == Some(PadAction::Back) {
                        return self.close_players();
                    }
                    if self.pads.iter().all(|p| screen.pressed.contains(p)) {
                        self.close_players();
                    }
                    return;
                }
                if let PadEvent::Pressed { button, .. } = ev
                    && self.controls.as_ref().is_some_and(|s| s.capturing)
                {
                    if let Some(button) = button {
                        self.press_for_controls(button);
                    }
                    return;
                }
                let Some(a) = ev.action() else { return };
                self.apply(match a {
                    PadAction::Up => Op::Up,
                    PadAction::Down => Op::Down,
                    PadAction::Left => Op::Left,
                    PadAction::Right => Op::Right,
                    PadAction::Confirm => Op::Confirm,
                    PadAction::Back => Op::Back,
                    PadAction::Favorite => Op::Favorite,
                    PadAction::Random => Op::Random,
                    PadAction::PrevSystem => Op::PrevSystem,
                    PadAction::NextSystem => Op::NextSystem,
                    PadAction::PrevLetter => Op::PrevLetter,
                    PadAction::NextLetter => Op::NextLetter,
                    PadAction::Menu => Op::Menu,
                    PadAction::Search => Op::Search,
                    PadAction::Filter => Op::Filter,
                    PadAction::ToggleGrid => Op::ToggleGrid,
                });
            }
        }
    }

    pub(super) fn apply(&mut self, op: Op) {
        // A path can't be entered with a controller, only given up on.
        if self.art_prompt.is_some() {
            if matches!(op, Op::Back | Op::Menu) {
                self.art_prompt = None;
            }
            return;
        }
        // The on-screen keyboard is up while a tag is named, over the game's tags.
        if self.tag_name.is_some() {
            return self.osk_op(op);
        }
        if self.stats.is_some() {
            if matches!(op, Op::Back | Op::Menu | Op::Confirm) {
                self.stats = None;
            }
            return;
        }
        if self.cheevos.is_some() {
            if self.osk.is_some() {
                return self.osk_op(op);
            }
            return self.cheevos_op(op);
        }
        if self.players.is_some() {
            if matches!(op, Op::Back | Op::Menu) {
                self.close_players();
            }
            return;
        }
        if self.controls.is_some() {
            return self.controls_op(op);
        }
        if self.filter_panel.is_some() {
            return self.filter_op(op);
        }
        if self.options.is_some() {
            return self.options_op(op);
        }
        if self.menu.is_some() {
            return self.menu_op(op);
        }
        if self.osk.is_some() {
            return self.osk_op(op);
        }
        let page = self.page.max(1) as isize;
        // In the cover grid, up and down go a row at a time, and left and right a cover, until
        // left runs out of row and goes to the systems, as in the list.
        let grid = self.grid_cols.max(1) as isize;
        let in_grid = self.state.grid && self.grid_cols > 0 && (self.focus == Pane::Games || self.search.is_some());
        match op {
            Op::Up if in_grid => self.move_cursor(-grid),
            Op::Down if in_grid => self.move_cursor(grid),
            Op::Left if in_grid && !self.game.is_multiple_of(self.grid_cols) => self.move_cursor(-1),
            Op::Right if in_grid => self.move_cursor(1),
            Op::Up => self.move_cursor(-1),
            Op::Down => self.move_cursor(1),
            Op::PageUp => self.move_cursor(-page),
            Op::PageDown => self.move_cursor(page),
            Op::Top => self.move_cursor(isize::MIN / 2),
            Op::Bottom => self.move_cursor(isize::MAX / 2),
            Op::Left => self.focus = Pane::Systems,
            Op::Right => self.focus = Pane::Games,
            Op::Confirm => match self.focus {
                Pane::Systems if self.search.is_none() => self.focus = Pane::Games,
                _ => {
                    if let Some(g) = self.selected_game() {
                        self.play(g);
                    }
                }
            },
            Op::Back => {
                if self.search.is_some() {
                    self.end_search();
                } else {
                    self.focus = Pane::Systems;
                }
            }
            Op::NextSystem | Op::PrevSystem => {
                self.end_search();
                if !self.views.is_empty() {
                    let n = self.views.len();
                    let to = if matches!(op, Op::NextSystem) { self.view + 1 } else { self.view + n - 1 };
                    self.switch_view(to % n);
                }
            }
            Op::Favorite => {
                if let Some(g) = self.selected_game() {
                    self.toggle_favorite(g);
                }
            }
            Op::Random => {
                let n = self.list().len();
                if n > 0 {
                    self.game = random_below(n);
                    self.focus = Pane::Games;
                }
            }
            Op::ToggleDetails => self.show_details = !self.show_details,
            Op::Achievements => self.cheevos = Some(crate::tui::cheevos::Screen::default()),
            Op::Stats => {
                let (history, now) = (crate::stats::history(), crate::state::now());
                self.stats = Some(crate::stats::Stats::compute(&self.lib, &self.state, &history, now));
            }
            Op::ToggleGrid => {
                let grid = !self.state.grid;
                self.state = State::update(|s| s.grid = grid);
                self.grid_top = 0;
            }
            Op::NextLetter => self.jump_letter(true),
            Op::PrevLetter => self.jump_letter(false),
            Op::Menu => self.menu = Some(Menu { game: None, selected: 0, tags: false }),
            Op::GameMenu => {
                if let Some(g) = self.selected_game() {
                    self.emulators.find(&self.cfg, &self.lib, g);
                    self.menu = Some(Menu { game: Some(g), selected: 0, tags: false });
                }
            }
            Op::Filter => self.filter_panel = Some(0),
            Op::Sort => self.cycle_sort(),
            Op::Search => {
                if self.search.is_none() {
                    self.start_search();
                }
                self.osk = Some((0, 0));
            }
        }
    }
}
