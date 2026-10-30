//! Game input: the keyboard and controllers, as a libretro pad's buttons, sticks and triggers.
//! Holding Select and Start together quits, from either.

use crate::emu::libretro::{Input, button};
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

    /// What's held on the keyboard and every controller together, or None once Select and Start
    /// have been held together long enough to quit.
    pub fn input(&mut self) -> Option<Input> {
        let mut input = self.keys.iter().fold(Input::default(), |mut input, &k| {
            input.buttons |= key_button(k).unwrap_or(0);
            add(&mut input.sticks[0], key_stick(k));
            input
        });
        if let Some(gilrs) = &mut self.gilrs {
            while gilrs.next_event().is_some() {}
            for (_, pad) in gilrs.gamepads() {
                let pad = pad_input(&pad);
                input.buttons |= pad.buttons;
                for (stick, more) in input.sticks.iter_mut().zip(pad.sticks) {
                    add(stick, more);
                }
                for (trigger, more) in input.triggers.iter_mut().zip(pad.triggers) {
                    *trigger = (*trigger).max(more);
                }
            }
        }
        let quit = button("select").unwrap() | button("start").unwrap();
        if input.buttons & quit == quit {
            if self.quit_held_since.get_or_insert_with(Instant::now).elapsed() >= QUIT_HOLD {
                return None;
            }
        } else {
            self.quit_held_since = None;
        }
        Some(input)
    }
}

/// Adds one stick's push to another's, as far as a stick goes.
fn add(stick: &mut [i16; 2], more: [i16; 2]) {
    for (axis, more) in stick.iter_mut().zip(more) {
        *axis = axis.saturating_add(more).clamp(-i16::MAX, i16::MAX);
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

/// What's held on one controller, in libretro's layout: A on the right face button, B on the
/// bottom one, as on a Super Nintendo pad.
fn pad_input(pad: &gilrs::Gamepad) -> Input {
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
    let buttons = MAP.iter().filter(|(b, _)| pad.is_pressed(*b)).fold(0, |acc, (_, n)| acc | button(n).unwrap());
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
    }
}
