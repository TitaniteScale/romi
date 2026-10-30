//! Remapped controls: which of libretro's buttons and stick directions each of the pad's sends, and
//! the type of controller plugged into each port. They come from RetroArch's remap file (see
//! `emucfg::retroarch_remap`), with the buttons remapped in romi over it: the system's, then the
//! game's, which go over every player's.

use crate::emu::libretro::{BUTTONS, Input, PLAYERS};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The sticks' directions, after the 16 buttons, in RetroArch's order and with its names. Y+ is
/// down, as in libretro.
const DIRECTIONS: [&str; 8] = ["l_x+", "l_x-", "l_y+", "l_y-", "r_x+", "r_x-", "r_y+", "r_y-"];

const CONTROLS: usize = BUTTONS.len() + DIRECTIONS.len();

/// L2's and R2's places among the buttons, which also have how far they're pressed.
const L2: usize = 12;
const R2: usize = 13;

/// What each button, then each stick direction, sends: another one's place, or None for nothing.
type Sends = [Option<u8>; CONTROLS];

#[derive(Clone, Debug, PartialEq)]
pub struct Remap {
    /// Each player's.
    players: [Sends; PLAYERS],
    /// The type of controller in each port (a `RETRO_DEVICE_*`), where the file sets one.
    pub devices: [Option<u32>; PLAYERS],
}

fn unchanged() -> Sends {
    std::array::from_fn(|i| Some(i as u8))
}

/// A button's or stick direction's name, by its place.
fn name(place: usize) -> &'static str {
    BUTTONS.get(place).or_else(|| DIRECTIONS.get(place - BUTTONS.len())).copied().unwrap_or("none")
}

fn place(name: &str) -> Option<usize> {
    BUTTONS.iter().chain(&DIRECTIONS).position(|n| *n == name)
}

/// Buttons remapped in romi: what each of the pad's buttons sends, by name, as a button, a stick
/// direction or "none". Only what differs from the remap below is kept.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Controls(pub BTreeMap<String, String>);

impl Controls {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for Controls {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Controls, D::Error> {
        let map = BTreeMap::<String, String>::deserialize(d)?;
        let buttons = BUTTONS.join(", ");
        for (from, to) in &map {
            if !BUTTONS.contains(&from.as_str()) {
                return Err(serde::de::Error::custom(format!("no button called \"{from}\" (there's {buttons})")));
            }
            if to != "none" && place(to).is_none() {
                let all = format!("{buttons}, {}, or none", DIRECTIONS.join(", "));
                return Err(serde::de::Error::custom(format!("{from} can't send \"{to}\" (it can send {all})")));
            }
        }
        Ok(Controls(map))
    }
}

impl Default for Remap {
    fn default() -> Remap {
        Remap { players: [unchanged(); PLAYERS], devices: [None; PLAYERS] }
    }
}

impl Remap {
    /// From a remap file's lines. What it leaves out stays as it is, and -1, or anything that isn't
    /// a button or direction, sends nothing, as in RetroArch.
    pub fn parse(lines: &BTreeMap<&str, &str>) -> Remap {
        let mut remap = Remap::default();
        for (player, sends) in remap.players.iter_mut().enumerate() {
            let names = BUTTONS.iter().map(|b| format!("btn_{b}")).chain(DIRECTIONS.iter().map(|d| format!("stk_{d}")));
            for (sends, name) in sends.iter_mut().zip(names) {
                let key = format!("input_player{}_{name}", player + 1);
                if let Some(n) = lines.get(key.as_str()).and_then(|v| v.parse::<i64>().ok()) {
                    *sends = u8::try_from(n).ok().filter(|&n| usize::from(n) < CONTROLS);
                }
            }
        }
        for (port, device) in remap.devices.iter_mut().enumerate() {
            *device = lines.get(format!("input_libretro_device_p{}", port + 1).as_str()).and_then(|v| v.parse().ok());
        }
        remap
    }

    /// With `controls` over every player's, button by button.
    pub fn with(mut self, controls: &Controls) -> Remap {
        for (from, to) in &controls.0 {
            if let Some(from) = place(from).filter(|&p| p < BUTTONS.len()) {
                for sends in &mut self.players {
                    sends[from] = place(to).map(|p| p as u8);
                }
            }
        }
        self
    }

    /// Player 1's buttons that send the button at `place` (see `libretro::BUTTONS`). The controls
    /// screen shows and changes player 1's, and saves them for everyone.
    pub fn sent_by(&self, place: usize) -> impl Iterator<Item = usize> + '_ {
        (0..BUTTONS.len()).filter(move |&b| self.players[0][b] == Some(place as u8))
    }

    /// Has player 1's button `pressed` send the button at `place`, and the button that sent it
    /// before send what `pressed` did, so the two swap and none is left doing nothing.
    pub fn swap(&mut self, place: usize, pressed: usize) {
        let before = self.sent_by(place).next();
        let sends = &mut self.players[0];
        let was = sends[pressed];
        sends[pressed] = Some(place as u8);
        if let Some(before) = before.filter(|&b| b != pressed) {
            sends[before] = was;
        }
    }

    /// Player 1's buttons that differ from `below`'s, to keep as romi's remap over it.
    pub fn controls_over(&self, below: &Remap) -> Controls {
        let (now, below) = (&self.players[0], &below.players[0]);
        let changed = (0..BUTTONS.len()).filter(|&b| now[b] != below[b]);
        Controls(
            changed.map(|b| (name(b).to_string(), name(now[b].map_or(usize::MAX, usize::from)).to_string())).collect(),
        )
    }

    /// What the core sees when `player` (from 0) holds `input`. A button sent to a stick direction
    /// pushes the stick all the way, and a direction sent to a button presses it once the stick is
    /// pushed halfway.
    pub fn apply(&self, player: usize, input: Input) -> Input {
        let Some(sends) = self.players.get(player).filter(|s| **s != unchanged()) else { return input };
        let mut out = Input::default();
        let mut pushed = [0i16; CONTROLS];
        for (from, to) in sends.iter().enumerate() {
            let Some(to) = to.map(usize::from) else { continue };
            let push = how_far(&input, from);
            pushed[to] = pushed[to].max(push);
            let pressed = match from {
                0..16 => input.buttons >> from & 1 == 1,
                _ => push > i16::MAX / 2,
            };
            if to < BUTTONS.len() && pressed {
                out.buttons |= 1 << to;
            }
        }
        for (stick, axes) in out.sticks.iter_mut().enumerate() {
            for (axis, value) in axes.iter_mut().enumerate() {
                let plus = BUTTONS.len() + stick * 4 + axis * 2;
                *value = pushed[plus] - pushed[plus + 1];
            }
        }
        out.triggers = [pushed[L2], pushed[R2]];
        out
    }
}

/// How far a button or stick direction is pushed, from 0 to 32767.
fn how_far(input: &Input, control: usize) -> i16 {
    let held = |b: usize| if input.buttons >> b & 1 == 1 { i16::MAX } else { 0 };
    match control {
        L2 => input.triggers[0].max(held(L2)),
        R2 => input.triggers[1].max(held(R2)),
        0..16 => held(control),
        _ => {
            let d = control - BUTTONS.len();
            let value = input.sticks[d / 4][d / 2 % 2];
            if d.is_multiple_of(2) { value.max(0) } else { value.saturating_neg().max(0) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emu::libretro::button;

    fn remap(text: &str) -> Remap {
        Remap::parse(&text.lines().filter_map(|l| l.split_once(" = ")).collect())
    }

    #[test]
    fn buttons_send_others_or_nothing() {
        // A and B swapped, Select sends nothing, and a number out of range sends nothing too.
        let r = remap(
            "input_player1_btn_a = 0\ninput_player1_btn_b = 8\ninput_player1_btn_select = -1\ninput_player1_btn_x = 99",
        );
        let held = |names: &[&str]| Input::buttons(names.iter().map(|n| button(n).unwrap()).fold(0, |a, b| a | b));
        assert_eq!(r.apply(0, held(&["a"])).buttons, button("b").unwrap());
        assert_eq!(r.apply(0, held(&["a", "b", "start"])), held(&["a", "b", "start"]));
        assert_eq!(r.apply(0, held(&["select", "x", "up"])), held(&["up"]));
    }

    #[test]
    fn sticks_swap_and_turn_into_buttons() {
        // The sticks swapped, except that the right stick's left is the d-pad's left instead.
        let r = remap(
            "input_player1_stk_l_x+ = 20\ninput_player1_stk_l_x- = 21\ninput_player1_stk_l_y+ = 22\ninput_player1_stk_l_y- = 23\n\
             input_player1_stk_r_x+ = 16\ninput_player1_stk_r_x- = 6\ninput_player1_stk_r_y+ = 18\ninput_player1_stk_r_y- = 19",
        );
        let input = Input { sticks: [[1000, -2000], [-30000, 500]], ..Input::default() };
        let out = r.apply(0, input);
        assert_eq!(out.sticks, [[0, 500], [1000, -2000]]);
        assert_eq!(out.buttons, button("left").unwrap());
        // Pushed only a little, it isn't pressed.
        assert_eq!(r.apply(0, Input { sticks: [[0; 2], [-1000, 0]], ..input }).buttons, 0);
    }

    #[test]
    fn triggers_go_with_their_buttons() {
        let r = remap("input_player1_btn_l2 = 13\ninput_player1_btn_r2 = 12\ninput_player1_btn_y = 17");
        let input = Input { triggers: [8000, 0], ..Input::buttons(button("y").unwrap()) };
        let out = r.apply(0, input);
        assert_eq!(out.triggers, [0, 8000]);
        // Y pushes the left stick all the way left.
        assert_eq!((out.buttons, out.sticks[0]), (0, [-i16::MAX, 0]));
    }

    #[test]
    fn romis_buttons_go_over_retroarchs_one_by_one() {
        let below = remap("input_player1_btn_a = 0\ninput_player1_btn_b = 8\ninput_player1_btn_l = 16");
        let controls: Controls = serde_json::from_str(r#"{ "a": "x", "select": "none" }"#).unwrap();
        let r = below.clone().with(&controls);
        assert_eq!(r.apply(0, Input::buttons(button("a").unwrap())).buttons, button("x").unwrap());
        // B still sends A, and L the left stick to the right, as RetroArch's remap has them.
        assert_eq!(r.apply(0, Input::buttons(button("b").unwrap())).buttons, button("a").unwrap());
        assert_eq!(r.apply(0, Input::buttons(button("l").unwrap())).sticks[0], [i16::MAX, 0]);
        assert_eq!(r.apply(0, Input::buttons(button("select").unwrap())).buttons, 0);
        assert_eq!(r.controls_over(&below), controls);
        assert!(below.controls_over(&below).is_empty());
    }

    #[test]
    fn swapping_leaves_no_button_doing_nothing() {
        let place = |n| BUTTONS.iter().position(|b| *b == n).unwrap();
        let mut r = Remap::default();
        // Y for A: Y sends A, and A sends what Y did.
        r.swap(place("a"), place("y"));
        assert_eq!(r.sent_by(place("a")).collect::<Vec<_>>(), [place("y")]);
        assert_eq!(r.sent_by(place("y")).collect::<Vec<_>>(), [place("a")]);
        let saved = r.controls_over(&Remap::default());
        assert_eq!(saved.0, BTreeMap::from([("a".into(), "y".into()), ("y".into(), "a".into())]));
        // Back again, there's nothing to keep.
        r.swap(place("a"), place("a"));
        assert!(r.controls_over(&Remap::default()).is_empty());
    }

    #[test]
    fn controls_name_real_buttons() {
        let err = |json| serde_json::from_str::<Controls>(json).unwrap_err().to_string();
        assert!(err(r#"{ "jump": "a" }"#).starts_with("no button called \"jump\""));
        assert!(err(r#"{ "a": "fire" }"#).starts_with("a can't send \"fire\""));
        assert!(serde_json::from_str::<Controls>(r#"{ "l3": "r_y-", "r3": "none" }"#).is_ok());
    }

    #[test]
    fn each_player_has_retroarchs_own_and_romis_go_over_all() {
        let r = remap("input_player1_btn_a = 0\ninput_player2_btn_a = 9");
        let a = Input::buttons(button("a").unwrap());
        let sent = |r: &Remap| [0, 1, 2].map(|p| r.apply(p, a).buttons);
        assert_eq!(sent(&r), [button("b").unwrap(), button("x").unwrap(), button("a").unwrap()]);
        let controls: Controls = serde_json::from_str(r#"{ "a": "start" }"#).unwrap();
        assert_eq!(sent(&r.with(&controls)), [button("start").unwrap(); 3]);
    }

    #[test]
    fn controller_types_by_port() {
        let r = remap("input_libretro_device_p1 = 517\ninput_libretro_device_p3 = 1");
        assert_eq!(r.devices, [Some(517), None, Some(1), None]);
        assert_eq!(r.apply(0, Input::buttons(5)), Input::buttons(5));
    }
}
