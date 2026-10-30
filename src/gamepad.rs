//! Controller input. Terminals never see gamepad events, so romi reads the device directly on a
//! background thread and turns button presses into navigation actions, with key-repeat for held
//! directions.

use gilrs::{Axis, Button, EventType, Gilrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

const REPEAT_DELAY: Duration = Duration::from_millis(320);
const REPEAT_EVERY: Duration = Duration::from_millis(55);
/// Holding a direction this long speeds the repeat up, for crossing long lists.
const REPEAT_FAST_AFTER: Duration = Duration::from_millis(1500);
const REPEAT_FAST: Duration = Duration::from_millis(22);
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
    PrevLetter,
    NextLetter,
    Menu,
    Search,
    Filter,
    ToggleGrid,
}

#[derive(Debug)]
pub enum PadEvent {
    /// A button going down: which of the pad's buttons it is to a game (see `emu::controls::PAD`),
    /// and what it does in romi's menus.
    Pressed(Option<&'static str>, Option<PadAction>),
    /// A held direction repeating, or the stick pushed.
    Action(PadAction),
    Connected(String),
    Disconnected,
    Unavailable(String),
}

impl PadEvent {
    /// What it does in romi's menus.
    pub fn action(&self) -> Option<PadAction> {
        match self {
            PadEvent::Pressed(_, action) => *action,
            PadEvent::Action(action) => Some(*action),
            _ => None,
        }
    }
}

impl PadAction {
    fn repeats(self) -> bool {
        matches!(self, Self::Up | Self::Down | Self::PrevLetter | Self::NextLetter)
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
        Button::South => Confirm,
        Button::Start | Button::Mode => Menu,
        Button::East => Back,
        Button::North => Favorite,
        Button::West => Random,
        Button::LeftTrigger => PrevSystem,
        Button::RightTrigger => NextSystem,
        Button::LeftTrigger2 => PrevLetter,
        Button::RightTrigger2 => NextLetter,
        Button::Select => Search,
        Button::RightThumb => Filter,
        Button::LeftThumb => ToggleGrid,
        _ => return None,
    })
}

/// Tracks the one direction currently held (button or stick) for repeat.
struct Held {
    action: PadAction,
    since: Instant,
    last: Instant,
}

/// Reads the controllers on a thread of its own. `swap_confirm` is read on each press, so the
/// options can change it.
pub fn spawn(tx: Sender<PadEvent>, swap_confirm: Arc<AtomicBool>) {
    std::thread::Builder::new().name("gamepad".into()).spawn(move || run(tx, &swap_confirm)).ok();
}

fn run(tx: Sender<PadEvent>, swap_confirm: &AtomicBool) {
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
        let timeout = if held.is_some() { REPEAT_FAST / 2 } else { Duration::from_millis(500) };
        let mut out = Vec::new();

        if let Some(ev) = gilrs.next_event_blocking(Some(timeout)) {
            match ev.event {
                EventType::ButtonPressed(b, _) => {
                    let action = map_button(b, swap_confirm.load(Ordering::Relaxed));
                    if let Some(a) = action.filter(|a| a.repeats()) {
                        held = Some(Held { action: a, since: Instant::now(), last: Instant::now() });
                    }
                    let button = crate::emu::controls::pad_name(b);
                    if button.is_some() || action.is_some() {
                        out.push(PadEvent::Pressed(button, action));
                    }
                }
                EventType::ButtonReleased(b, _) => {
                    if held
                        .as_ref()
                        .is_some_and(|h| Some(h.action) == map_button(b, swap_confirm.load(Ordering::Relaxed)))
                    {
                        held = None;
                    }
                }
                EventType::AxisChanged(Axis::LeftStickY, v, _) => {
                    let dir = stick(v, stick_y, PadAction::Up, PadAction::Down);
                    if dir != stick_y {
                        stick_y = dir;
                        held = None;
                        if let Some(a) = dir {
                            out.push(PadEvent::Action(a));
                            held = Some(Held { action: a, since: Instant::now(), last: Instant::now() });
                        }
                    }
                }
                EventType::AxisChanged(Axis::LeftStickX, v, _) => {
                    let dir = stick(v, stick_x, PadAction::Right, PadAction::Left);
                    if dir != stick_x {
                        stick_x = dir;
                        out.extend(dir.map(PadEvent::Action));
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
            let held_for = now.duration_since(h.since);
            let every = if held_for >= REPEAT_FAST_AFTER { REPEAT_FAST } else { REPEAT_EVERY };
            if held_for >= REPEAT_DELAY && now.duration_since(h.last) >= every {
                h.last = now;
                out.push(PadEvent::Action(h.action));
            }
        }

        for ev in out {
            if tx.send(ev).is_err() {
                return; // UI is gone
            }
        }
    }
}

/// Presses closer together than this count as one, so a bouncy button doesn't run it twice.
const GUIDE_DEBOUNCE: Duration = Duration::from_secs(1);

/// `romi guide`: runs `command` each time a controller's Guide button is pressed, until killed.
/// The controller is read the same way the TUI reads it, without taking it from anything else,
/// so it keeps working everywhere. While a game started from romi is running, Guide is the
/// game's (or Steam's overlay's), and nothing is run.
pub fn run_on_guide(command: &[String]) -> anyhow::Result<()> {
    let mut gilrs = Gilrs::new().map_err(|e| anyhow::anyhow!("can't read controllers: {e}"))?;
    let mut last: Option<Instant> = None;
    loop {
        let Some(ev) = gilrs.next_event_blocking(None) else { continue };
        if !matches!(ev.event, EventType::ButtonPressed(Button::Mode, _)) {
            continue;
        }
        if last.is_some_and(|t| t.elapsed() < GUIDE_DEBOUNCE) || !crate::session::running().is_empty() {
            continue;
        }
        last = Some(Instant::now());
        match std::process::Command::new(&command[0]).args(&command[1..]).stdin(std::process::Stdio::null()).spawn() {
            // Reaped on a thread of its own, so a slow command doesn't hold up the next press.
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(e) => eprintln!("romi guide: {}: {e}", command[0]),
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
