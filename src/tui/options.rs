//! The options panel: settings from the config that are worth changing from the couch, each with
//! the values it steps through. Changing one saves it into the config as you wrote it (see
//! `config_edit`), keeping your comments.

use crate::config::Config;
use crate::emu::screen::Filter;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Setting {
    Volume,
    Corners,
    Filter,
    Animation,
    Covers,
    Screens,
    Accent,
    Borders,
    Confirm,
}

pub const SETTINGS: [Setting; 9] = [
    Setting::Volume,
    Setting::Corners,
    Setting::Filter,
    Setting::Animation,
    Setting::Covers,
    Setting::Screens,
    Setting::Accent,
    Setting::Borders,
    Setting::Confirm,
];

const ACCENTS: [&str; 7] = ["blue", "cyan", "green", "yellow", "magenta", "red", "white"];
const BORDERS: [&str; 5] = ["rounded", "plain", "thick", "double", "none"];

impl Setting {
    pub fn label(self) -> &'static str {
        match self {
            Setting::Volume => "Volume",
            Setting::Corners => "Corners",
            Setting::Filter => "Screen filter",
            Setting::Animation => "Launch animation",
            Setting::Covers => "Covers",
            Setting::Screens => "Title screens",
            Setting::Accent => "Accent color",
            Setting::Borders => "Borders",
            Setting::Confirm => "Confirm button",
        }
    }

    /// Where it is in the config.
    pub fn path(self) -> &'static [&'static str] {
        match self {
            Setting::Volume => &["volume"],
            Setting::Corners => &["corner_radius"],
            Setting::Filter => &["screen_filter"],
            Setting::Animation => &["launch_animation", "enabled"],
            Setting::Covers => &["art", "show"],
            Setting::Screens => &["art", "screens"],
            Setting::Accent => &["theme", "accent"],
            Setting::Borders => &["theme", "borders"],
            Setting::Confirm => &["controller", "swap_confirm"],
        }
    }

    /// What it's set to now.
    fn current(self, cfg: &Config) -> Value {
        match self {
            Setting::Volume => json!(cfg.volume),
            Setting::Corners => json!(cfg.corner_radius),
            Setting::Filter => json!(cfg.screen_filter.name()),
            Setting::Animation => json!(cfg.launch_animation.enabled),
            Setting::Covers => json!(cfg.art.show),
            Setting::Screens => json!(cfg.art.screens),
            Setting::Accent => json!(cfg.theme.accent),
            Setting::Borders => json!(cfg.theme.borders),
            Setting::Confirm => json!(cfg.controller.swap_confirm),
        }
    }

    /// The values it steps through, in order.
    fn choices(self) -> Vec<Value> {
        match self {
            Setting::Volume => (0..=10).map(|n| json!(n * 10)).collect(),
            Setting::Corners => [0, 2, 4, 6, 8].map(|n| json!(n)).to_vec(),
            Setting::Filter => Filter::ALL.map(|f| json!(f.name())).to_vec(),
            Setting::Accent => ACCENTS.map(|a| json!(a)).to_vec(),
            Setting::Borders => BORDERS.map(|b| json!(b)).to_vec(),
            Setting::Animation | Setting::Covers | Setting::Screens | Setting::Confirm => {
                vec![json!(true), json!(false)]
            }
        }
    }

    /// The value after the current one (or before it), wrapping round. A number between two
    /// choices goes to the next one along; anything else not among them starts from the first.
    pub fn step(self, cfg: &Config, forward: bool) -> Value {
        let choices = self.choices();
        let now = self.current(cfg);
        let n = choices.len();
        if let Some(at) = choices.iter().position(|c| *c == now) {
            return choices[if forward { (at + 1) % n } else { (at + n - 1) % n }].clone();
        }
        let Some(x) = now.as_f64() else { return choices[0].clone() };
        let past = |c: &&Value| c.as_f64().is_some_and(|c| if forward { c > x } else { c < x });
        let found = if forward { choices.iter().find(past) } else { choices.iter().rev().find(past) };
        found.unwrap_or(if forward { &choices[0] } else { &choices[n - 1] }).clone()
    }

    /// What it's set to, for the panel.
    pub fn shown(self, cfg: &Config) -> String {
        let on = |b: bool| if b { "on" } else { "off" }.to_string();
        match self {
            Setting::Volume if cfg.volume == 0 => "muted".into(),
            Setting::Volume => format!("{}%", cfg.volume.min(100)),
            Setting::Corners if cfg.corner_radius == 0 => "square".into(),
            Setting::Corners => format!("round, {}%", cfg.corner_radius.min(50)),
            Setting::Filter => cfg.screen_filter.name().into(),
            Setting::Animation => on(cfg.launch_animation.enabled),
            Setting::Covers => on(cfg.art.show),
            Setting::Screens => on(cfg.art.screens),
            Setting::Accent => cfg.theme.accent.clone(),
            Setting::Borders => cfg.theme.borders.clone(),
            Setting::Confirm if cfg.controller.swap_confirm => "B (A goes back)".into(),
            Setting::Confirm => "A (B goes back)".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_through_the_choices_both_ways() {
        let mut cfg = Config::default();
        assert_eq!(cfg.volume, 100);
        assert_eq!(Setting::Volume.step(&cfg, true), json!(0), "past the end, round to the start");
        assert_eq!(Setting::Volume.step(&cfg, false), json!(90));
        // Between choices, to the next one along.
        cfg.volume = 75;
        assert_eq!((Setting::Volume.step(&cfg, true), Setting::Volume.step(&cfg, false)), (json!(80), json!(70)));
        assert_eq!(Setting::Animation.step(&cfg, true), json!(false));
        cfg.theme.accent = "#ff8800".into();
        assert_eq!(Setting::Accent.step(&cfg, true), json!("blue"));
    }

    #[test]
    fn every_setting_saves_into_the_default_config() {
        for setting in SETTINGS {
            let cfg = crate::config::parse(crate::config::DEFAULT_CONFIG).unwrap();
            let value = setting.step(&cfg, true);
            let edited = crate::config_edit::set(crate::config::DEFAULT_CONFIG, setting.path(), &value)
                .unwrap_or_else(|| panic!("{setting:?}"));
            let cfg = crate::config::parse(&edited).unwrap();
            assert_eq!(setting.current(&cfg), value, "{setting:?}");
        }
    }
}
