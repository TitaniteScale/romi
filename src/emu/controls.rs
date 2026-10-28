//! Game input: the keyboard and controllers, as libretro joypad buttons. Holding Select and Start
//! together quits, from either.

use crate::emu::libretro::button;
use gilrs::{Axis, Button, Gilrs};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Holding Select and Start together this long quits.
const QUIT_HOLD: Duration = Duration::from_secs(1);

pub struct Controls {
    gilrs: Option<Gilrs>,
    /// Keys held down. Releases only arrive in terminals with the Kitty keyboard protocol.
    keys: HashSet<KeyCode>,
    quit_held_since: Option<Instant>,
}

impl Controls {
    pub fn new() -> Controls {
        Controls { gilrs: Gilrs::new().ok(), keys: HashSet::new(), quit_held_since: None }
    }

    /// Notes a key going down or coming up.
    pub fn key(&mut self, k: KeyEvent) {
        if k.kind == KeyEventKind::Release {
            self.keys.remove(&k.code);
        } else {
            self.keys.insert(k.code);
        }
    }

    /// The buttons held on the keyboard and every controller, or None once Select and Start have
    /// been held together long enough to quit.
    pub fn buttons(&mut self) -> Option<u16> {
        let mut buttons = self.keys.iter().filter_map(|&k| key_button(k)).fold(0, |b, n| b | n);
        if let Some(gilrs) = &mut self.gilrs {
            while gilrs.next_event().is_some() {}
            for (_, pad) in gilrs.gamepads() {
                buttons |= pad_buttons(&pad);
            }
        }
        let quit = button("select").unwrap() | button("start").unwrap();
        if buttons & quit == quit {
            if self.quit_held_since.get_or_insert_with(Instant::now).elapsed() >= QUIT_HOLD {
                return None;
            }
        } else {
            self.quit_held_since = None;
        }
        Some(buttons)
    }
}

/// RetroArch's default keys: arrows, X and Z for A and B, A and S for L and R, Enter for Start,
/// and Backspace for Select (RetroArch uses right Shift, which terminals don't report alone).
fn key_button(code: KeyCode) -> Option<u16> {
    let name = match code {
        KeyCode::Up => "up",
        KeyCode::Down => "down",
        KeyCode::Left => "left",
        KeyCode::Right => "right",
        KeyCode::Char('x' | 'X') => "a",
        KeyCode::Char('z' | 'Z') => "b",
        KeyCode::Char('s' | 'S') => "x",
        KeyCode::Char('a' | 'A') => "y",
        KeyCode::Char('q' | 'Q') => "l",
        KeyCode::Char('w' | 'W') => "r",
        KeyCode::Enter => "start",
        KeyCode::Backspace => "select",
        _ => return None,
    };
    button(name)
}

/// The buttons held on one controller, in libretro's layout: A on the right face button, B on
/// the bottom one, as on a Super Nintendo pad. The left stick works as the d-pad.
fn pad_buttons(pad: &gilrs::Gamepad) -> u16 {
    const MAP: [(Button, &str); 14] = [
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
    ];
    let mut buttons = MAP.iter().filter(|(b, _)| pad.is_pressed(*b)).fold(0, |acc, (_, n)| acc | button(n).unwrap());
    let (x, y) = (pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY));
    for (held, name) in [(x < -0.5, "left"), (x > 0.5, "right"), (y > 0.5, "up"), (y < -0.5, "down")] {
        if held {
            buttons |= button(name).unwrap();
        }
    }
    buttons
}
