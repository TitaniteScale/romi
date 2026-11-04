//! Game input: the keyboard and controllers, as libretro pads' buttons, sticks and triggers. Each
//! controller is a player of its own, in the order saved from the players screen (`State::players`),
//! then in the order romi finds them; the keyboard plays as player 1 too. Holding Select and Start
//! together quits, from any of them.

use crate::config::StateButton;
use crate::emu::libretro::{Input, PLAYERS, Players, button};
use gilrs::{Axis, Button, EventType, GamepadId, Gilrs};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Holding Select and Start together this long quits.
const QUIT_HOLD: Duration = Duration::from_secs(1);

/// A button pressed while the state button is held (`config::StateButton`), on any controller. These
/// buttons don't reach the game meanwhile. Guide never does; Select does, as romi can't know what's
/// coming.
pub const CHORDS: [(&str, Chord); 5] = [
    ("r", Chord::SaveState),
    ("l", Chord::LoadState),
    ("down", Chord::UndoLoad),
    ("right", Chord::NextSlot),
    ("left", Chord::PreviousSlot),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Chord {
    SaveState,
    LoadState,
    UndoLoad,
    NextSlot,
    PreviousSlot,
}

/// Takes the chords' buttons out of what the players holding the state button hold (`holding`, by
/// player), giving back what they held.
fn held_chords(players: &mut Players, holding: [bool; PLAYERS]) -> u16 {
    let chords = CHORDS.iter().fold(0, |bits, (name, _)| bits | button(name).unwrap());
    let mut held = 0;
    for (player, _) in players.iter_mut().zip(holding).filter(|(_, holding)| *holding) {
        held |= player.buttons & chords;
        player.buttons &= !chords;
    }
    held
}

/// Whether a player is holding the state button, with `guide` for whether their controller has a Guide
/// button and it's held, as None without one, and whether they hold Select. The keyboard has no Guide.
fn holds_state_button(setting: StateButton, guide: Option<bool>, select: bool) -> bool {
    match (setting, guide) {
        (StateButton::Off, _) => false,
        (StateButton::Guide, Some(held)) => held,
        (StateButton::Guide, None) | (StateButton::Select, _) => select,
    }
}

pub struct Controls {
    gilrs: Option<Gilrs>,
    /// The controllers, player 1's first, worked out again when one comes or goes.
    order: Vec<GamepadId>,
    /// The order saved from the players screen, by `pad_names`.
    saved: Vec<String>,
    /// Keys held down. Releases only arrive in terminals with the Kitty keyboard protocol.
    keys: HashSet<KeyCode>,
    quit_held_since: Option<Instant>,
    /// The chords' buttons held as of the last look, so holding one does it once.
    chords_held: u16,
    /// Chords pressed and not yet taken.
    chords: Vec<Chord>,
    state_button: StateButton,
}

impl Controls {
    pub fn new(state_button: StateButton) -> Controls {
        let gilrs = Gilrs::new().ok();
        let saved = crate::state::State::load().players;
        let order = gilrs.as_ref().map(|g| player_order(g, &saved)).unwrap_or_default();
        Controls {
            gilrs,
            order,
            saved,
            keys: HashSet::new(),
            quit_held_since: None,
            chords_held: 0,
            chords: Vec::new(),
            state_button,
        }
    }

    /// Notes a key going down or coming up.
    pub fn key(&mut self, k: KeyEvent) {
        if k.kind == KeyEventKind::Release {
            self.keys.remove(&k.code);
        } else {
            self.keys.insert(k.code);
        }
    }

    /// The chords pressed since the last time.
    pub fn take_chords(&mut self) -> Vec<Chord> {
        std::mem::take(&mut self.chords)
    }

    /// What each player holds, or None once one has held Select and Start together long enough
    /// to quit.
    pub fn input(&mut self) -> Option<Players> {
        let mut players = Players::default();
        for &k in &self.keys {
            players[0].buttons |= key_button(k).unwrap_or(0);
            add(&mut players[0].sticks[0], key_stick(k));
        }
        if let Some(gilrs) = &mut self.gilrs {
            let mut changed = false;
            while let Some(ev) = gilrs.next_event() {
                changed |= matches!(ev.event, EventType::Connected | EventType::Disconnected);
            }
            if changed {
                self.order = player_order(gilrs, &self.saved);
            }
            for (player, &id) in players.iter_mut().zip(&self.order) {
                merge(player, pad_input(&gilrs.gamepad(id)));
            }
        }
        let select = button("select").unwrap();
        // The keyboard is player 1. Its Backspace is Select, and it has no Guide to hold, but it
        // has its own keys for all of this.
        let keys_select = self.keys.iter().any(|&k| key_button(k).is_some_and(|b| b & select != 0));
        let mut holding = [false; PLAYERS];
        holding[0] = self.state_button == StateButton::Select && keys_select;
        if let Some(gilrs) = &self.gilrs {
            for (i, &id) in self.order.iter().enumerate().take(PLAYERS) {
                let pad = gilrs.gamepad(id);
                let guide = pad.button_code(Button::Mode).map(|_| pad.is_pressed(Button::Mode));
                let select = pad.is_pressed(Button::Select);
                holding[i] |= holds_state_button(self.state_button, guide, select);
            }
        }
        let held = held_chords(&mut players, holding);
        let pressed = held & !self.chords_held;
        self.chords_held = held;
        self.chords.extend(CHORDS.iter().filter(|(name, _)| pressed & button(name).unwrap() != 0).map(|(_, c)| *c));
        let quit = button("select").unwrap() | button("start").unwrap();
        if players.iter().any(|p| p.buttons & quit == quit) {
            if self.quit_held_since.get_or_insert_with(Instant::now).elapsed() >= QUIT_HOLD {
                return None;
            }
        } else {
            self.quit_held_since = None;
        }
        Some(players)
    }
}

/// The connected controllers by name, in the order gilrs lists them, a second of the same kind
/// called "… #2", and so on, so the players screen and a game tell them apart the same way.
pub fn pad_names(gilrs: &Gilrs) -> Vec<(GamepadId, String)> {
    let mut named: Vec<(GamepadId, String)> = Vec::new();
    for (id, pad) in gilrs.gamepads() {
        let name = pad.name().to_string();
        let same = named.iter().filter(|(_, n)| *n == name || n.starts_with(&format!("{name} #"))).count();
        named.push((id, if same == 0 { name } else { format!("{name} #{}", same + 1) }));
    }
    named
}

/// The controllers, player 1's first: the ones in `saved` in its order, then the rest as gilrs
/// lists them. Only as many as there are players.
fn player_order(gilrs: &Gilrs, saved: &[String]) -> Vec<GamepadId> {
    let mut pads = pad_names(gilrs);
    pads.sort_by_key(|(_, name)| saved.iter().position(|s| s == name).unwrap_or(usize::MAX));
    pads.into_iter().take(PLAYERS).map(|(id, _)| id).collect()
}

/// Adds what one more pad holds to what a player holds.
fn merge(input: &mut Input, more: Input) {
    input.buttons |= more.buttons;
    for (stick, more) in input.sticks.iter_mut().zip(more.sticks) {
        add(stick, more);
    }
    for (trigger, more) in input.triggers.iter_mut().zip(more.triggers) {
        *trigger = (*trigger).max(more);
    }
}

/// Adds one stick's push to another's, as far as a stick goes.
fn add(stick: &mut [i16; 2], more: [i16; 2]) {
    for (axis, more) in stick.iter_mut().zip(more) {
        *axis = axis.saturating_add(more).clamp(-i16::MAX, i16::MAX);
    }
}

/// The keys that are the pad's buttons, by the buttons' names (see `libretro::BUTTONS`): RetroArch's
/// default keys, arrows, X and Z for A and B, Q and W for L and R, Enter for Start, and Backspace for
/// Select (RetroArch uses right Shift, which terminals don't report alone).
pub const KEYS: [(&str, &str); 12] = [
    ("up", "↑"),
    ("down", "↓"),
    ("left", "←"),
    ("right", "→"),
    ("a", "x"),
    ("b", "z"),
    ("x", "s"),
    ("y", "a"),
    ("l", "q"),
    ("r", "w"),
    ("start", "enter"),
    ("select", "backspace"),
];

fn key_button(code: KeyCode) -> Option<u16> {
    button(key_name(code)?)
}

/// The pad's button a key is.
pub fn key_name(code: KeyCode) -> Option<&'static str> {
    let key = match code {
        KeyCode::Up => "↑".to_string(),
        KeyCode::Down => "↓".to_string(),
        KeyCode::Left => "←".to_string(),
        KeyCode::Right => "→".to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Char(c) => c.to_ascii_lowercase().to_string(),
        _ => return None,
    };
    KEYS.iter().find(|(_, k)| *k == key).map(|(name, _)| *name)
}

/// The arrows also push the left stick all the way, so a keyboard can walk in 3D games.
fn key_stick(code: KeyCode) -> [i16; 2] {
    match code {
        KeyCode::Up => [0, -i16::MAX],
        KeyCode::Down => [0, i16::MAX],
        KeyCode::Left => [-i16::MAX, 0],
        KeyCode::Right => [i16::MAX, 0],
        _ => [0, 0],
    }
}

/// A controller's buttons, by the names games know them by: libretro's layout, with A on the right
/// face button and B on the bottom one, as on a Super Nintendo pad.
pub const PAD: [(Button, &str); 16] = [
    (Button::East, "a"),
    (Button::South, "b"),
    (Button::North, "x"),
    (Button::West, "y"),
    (Button::LeftTrigger, "l"),
    (Button::RightTrigger, "r"),
    (Button::LeftTrigger2, "l2"),
    (Button::RightTrigger2, "r2"),
    (Button::Select, "select"),
    (Button::Start, "start"),
    (Button::DPadUp, "up"),
    (Button::DPadDown, "down"),
    (Button::DPadLeft, "left"),
    (Button::DPadRight, "right"),
    (Button::LeftThumb, "l3"),
    (Button::RightThumb, "r3"),
];

/// The pad's button a controller's button is.
pub fn pad_name(b: Button) -> Option<&'static str> {
    PAD.iter().find(|(p, _)| *p == b).map(|(_, n)| *n)
}

/// What's held on one controller.
fn pad_input(pad: &gilrs::Gamepad) -> Input {
    let buttons = PAD.iter().filter(|(b, _)| pad.is_pressed(*b)).fold(0, |acc, (_, n)| acc | button(n).unwrap());
    let axis = |a| (pad.value(a).clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
    // gilrs has up as positive; libretro has down.
    let stick = |x, y| [axis(x), axis(y).saturating_neg()];
    let trigger = |b| {
        let pressed = pad.button_data(b).map_or(0.0, |d| d.value());
        (pressed.clamp(0.0, 1.0) * f32::from(i16::MAX)) as i16
    };
    Input {
        buttons,
        sticks: [stick(Axis::LeftStickX, Axis::LeftStickY), stick(Axis::RightStickX, Axis::RightStickY)],
        triggers: [trigger(Button::LeftTrigger2), trigger(Button::RightTrigger2)],
        touch: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_buttons_stay_out_of_the_game_only_while_the_state_button_is_held() {
        let bits = |names: &[&str]| names.iter().fold(0, |b, n| b | button(n).unwrap());
        let mut players = Players::default();
        players[0].buttons = bits(&["select", "r", "left", "a"]);
        players[1].buttons = bits(&["r", "left"]);
        assert_eq!(held_chords(&mut players, [true, false, false, false]), bits(&["r", "left"]));
        assert_eq!(players[0].buttons, bits(&["select", "a"]));
        assert_eq!(players[1].buttons, bits(&["r", "left"]), "player 2 isn't holding it");
    }

    #[test]
    fn the_state_button_is_guide_or_select_and_can_be_off() {
        use StateButton::{Guide, Off, Select};
        assert!(holds_state_button(Guide, Some(true), false));
        assert!(!holds_state_button(Guide, Some(false), true), "Select is the game's, with a Guide to hold");
        assert!(holds_state_button(Guide, None, true), "without a Guide, Select does");
        assert!(!holds_state_button(Guide, None, false));
        assert!(holds_state_button(Select, Some(false), true));
        assert!(!holds_state_button(Select, Some(true), false));
        assert!(!holds_state_button(Off, Some(true), true));
    }
}
