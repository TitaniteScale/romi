//! Controller input. Terminals never see gamepad events, so romi reads the device directly on a
//! background thread and turns button presses into navigation actions, with key-repeat for held
//! directions.

use gilrs::{Axis, Button, EventType, Gilrs};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

const REPEAT_DELAY: Duration = Duration::from_millis(320);
const REPEAT_EVERY: Duration = Duration::from_millis(55);
const STICK_ON: f32 = 0.6;
const STICK_OFF: f32 = 0.3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadAction {
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Back,
    Favorite,
    Random,
    PrevSystem,
    NextSystem,
    PageUp,
    PageDown,
    ToggleDetails,
}

#[derive(Debug)]
pub enum PadEvent {
    Action(PadAction),
    Connected(String),
    Disconnected,
    Unavailable(String),
}

impl PadAction {
    fn repeats(self) -> bool {
        matches!(self, Self::Up | Self::Down | Self::PageUp | Self::PageDown)
    }
}

fn map_button(b: Button, swap_confirm: bool) -> Option<PadAction> {
    use PadAction::*;
    Some(match b {
        Button::DPadUp => Up,
        Button::DPadDown => Down,
        Button::DPadLeft => Left,
        Button::DPadRight => Right,
        Button::South if swap_confirm => Back,
        Button::East if swap_confirm => Confirm,
        Button::South | Button::Start => Confirm,
        Button::East => Back,
        Button::North => Favorite,
        Button::West => Random,
        Button::LeftTrigger => PrevSystem,
        Button::RightTrigger => NextSystem,
        Button::LeftTrigger2 => PageUp,
        Button::RightTrigger2 => PageDown,
        Button::Select => ToggleDetails,
        _ => return None,
    })
}

/// Tracks the one direction currently held (button or stick) for repeat.
struct Held {
    action: PadAction,
    since: Instant,
    last: Instant,
}

pub fn spawn(tx: Sender<PadEvent>, swap_confirm: bool) {
    std::thread::Builder::new().name("gamepad".into()).spawn(move || run(tx, swap_confirm)).ok();
}

fn run(tx: Sender<PadEvent>, swap_confirm: bool) {
    let mut gilrs = match Gilrs::new() {
        Ok(g) => g,
        Err(e) => {
            let _ = tx.send(PadEvent::Unavailable(e.to_string()));
            return;
        }
    };
    if let Some((_, pad)) = gilrs.gamepads().next() {
        let _ = tx.send(PadEvent::Connected(pad.name().to_string()));
    }

    let mut held: Option<Held> = None;
    let mut stick_y: Option<PadAction> = None;
    let mut stick_x: Option<PadAction> = None;

    loop {
        let timeout = if held.is_some() { REPEAT_EVERY / 2 } else { Duration::from_millis(500) };
        let mut out = Vec::new();

        if let Some(ev) = gilrs.next_event_blocking(Some(timeout)) {
            match ev.event {
                EventType::ButtonPressed(b, _) => {
                    if let Some(a) = map_button(b, swap_confirm) {
                        out.push(a);
                        if a.repeats() {
                            held = Some(Held { action: a, since: Instant::now(), last: Instant::now() });
                        }
                    }
                }
                EventType::ButtonReleased(b, _) => {
                    if held.as_ref().is_some_and(|h| Some(h.action) == map_button(b, swap_confirm)) {
                        held = None;
                    }
                }
                EventType::AxisChanged(Axis::LeftStickY, v, _) => {
                    let dir = stick(v, stick_y, PadAction::Up, PadAction::Down);
                    if dir != stick_y {
                        stick_y = dir;
                        held = None;
                        if let Some(a) = dir {
                            out.push(a);
                            held = Some(Held { action: a, since: Instant::now(), last: Instant::now() });
                        }
                    }
                }
                EventType::AxisChanged(Axis::LeftStickX, v, _) => {
                    let dir = stick(v, stick_x, PadAction::Right, PadAction::Left);
                    if dir != stick_x {
                        stick_x = dir;
                        out.extend(dir);
                    }
                }
                EventType::Connected => {
                    let name = gilrs.gamepad(ev.id).name().to_string();
                    let _ = tx.send(PadEvent::Connected(name));
                }
                EventType::Disconnected => {
                    held = None;
                    let event = match gilrs.gamepads().next() {
                        Some((_, pad)) => PadEvent::Connected(pad.name().to_string()),
                        None => PadEvent::Disconnected,
                    };
                    let _ = tx.send(event);
                }
                _ => {}
            }
        }

        if let Some(h) = &mut held {
            let now = Instant::now();
            if now.duration_since(h.since) >= REPEAT_DELAY && now.duration_since(h.last) >= REPEAT_EVERY {
                h.last = now;
                out.push(h.action);
            }
        }

        for a in out {
            if tx.send(PadEvent::Action(a)).is_err() {
                return; // UI is gone
            }
        }
    }
}

/// Converts a stick axis into a direction with hysteresis so it doesn't flicker at the edge.
fn stick(v: f32, current: Option<PadAction>, pos: PadAction, neg: PadAction) -> Option<PadAction> {
    if v >= STICK_ON {
        Some(pos)
    } else if v <= -STICK_ON {
        Some(neg)
    } else if v.abs() <= STICK_OFF {
        None
    } else {
        current
    }
}
