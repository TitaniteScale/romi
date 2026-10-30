//! Core options: the settings a libretro core declares, each with the values it can take and a
//! default, and asks for by key as it runs. romi answers with what's set for the core in RetroArch
//! (see `emucfg::retroarch_core_options`), so a game plays the same in both, or else the core's
//! default.
//!
//! The exception is how big a 3D core draws. Drawn at the console's size (640×480) and scaled
//! up to the panel, a pixel at a time, 3D games look blocky, so romi asks for the size closest to
//! the panel's instead, when it's told how tall that is, up to `MAX_DRAWN`.

use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_void};

/// libretro.h's `retro_variable`: a key the core asks about, and the value the frontend gives.
#[repr(C)]
pub struct Variable {
    pub key: *const c_char,
    pub value: *const c_char,
}

#[repr(C)]
struct OptionValue {
    value: *const c_char,
    label: *const c_char,
}

const VALUES_MAX: usize = 128;

/// `retro_core_option_definition`, from `SET_CORE_OPTIONS`.
#[repr(C)]
struct DefinitionV1 {
    key: *const c_char,
    desc: *const c_char,
    info: *const c_char,
    values: [OptionValue; VALUES_MAX],
    default_value: *const c_char,
}

/// `retro_core_options_intl`: the definitions in English, and maybe the user's language.
#[repr(C)]
struct IntlV1 {
    us: *const DefinitionV1,
    local: *const DefinitionV1,
}

/// `retro_core_option_v2_definition`, from `SET_CORE_OPTIONS_V2`.
#[repr(C)]
struct DefinitionV2 {
    key: *const c_char,
    desc: *const c_char,
    desc_categorized: *const c_char,
    info: *const c_char,
    info_categorized: *const c_char,
    category_key: *const c_char,
    values: [OptionValue; VALUES_MAX],
    default_value: *const c_char,
}

#[repr(C)]
struct OptionsV2 {
    categories: *const c_void,
    definitions: *const DefinitionV2,
}

#[repr(C)]
struct IntlV2 {
    us: *const OptionsV2,
    local: *const OptionsV2,
}

/// An option as a core declared it.
#[derive(Debug, PartialEq)]
pub struct Declared {
    pub key: String,
    /// What it can be set to, the default first unless `default` says otherwise.
    pub values: Vec<String>,
    pub default: Option<String>,
}

fn text(p: *const c_char) -> Option<String> {
    // SAFETY: the core's strings are NUL-terminated, valid while the call that gave them lasts.
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn values(values: &[OptionValue; VALUES_MAX]) -> Vec<String> {
    values.iter().map_while(|v| text(v.value)).collect()
}

/// Reads the options a core declared with libretro's environment call `cmd` (as `SET_VARIABLES`,
/// `SET_CORE_OPTIONS`, their `_INTL` forms or `_V2`), given `data`, what it points to.
///
/// # Safety
///
/// `data` must be what the core passed with `cmd`, as libretro.h describes it.
pub unsafe fn declared(cmd: u32, data: *const c_void) -> Vec<Declared> {
    // SAFETY: each list ends with an entry whose key is null, as libretro.h says.
    unsafe {
        let mut out = Vec::new();
        match cmd {
            super::libretro::ENV_SET_VARIABLES => {
                let mut v = data as *const Variable;
                while let Some(key) = text((*v).key) {
                    out.push(from_legacy(key, &text((*v).value).unwrap_or_default()));
                    v = v.add(1);
                }
            }
            super::libretro::ENV_SET_CORE_OPTIONS | super::libretro::ENV_SET_CORE_OPTIONS_INTL => {
                let mut d = match cmd {
                    super::libretro::ENV_SET_CORE_OPTIONS => data as *const DefinitionV1,
                    _ => (*(data as *const IntlV1)).us,
                };
                while !d.is_null()
                    && let Some(key) = text((*d).key)
                {
                    out.push(Declared { key, values: values(&(*d).values), default: text((*d).default_value) });
                    d = d.add(1);
                }
            }
            super::libretro::ENV_SET_CORE_OPTIONS_V2 | super::libretro::ENV_SET_CORE_OPTIONS_V2_INTL => {
                let options = match cmd {
                    super::libretro::ENV_SET_CORE_OPTIONS_V2 => data as *const OptionsV2,
                    _ => (*(data as *const IntlV2)).us,
                };
                let mut d = if options.is_null() { std::ptr::null() } else { (*options).definitions };
                while !d.is_null()
                    && let Some(key) = text((*d).key)
                {
                    out.push(Declared { key, values: values(&(*d).values), default: text((*d).default_value) });
                    d = d.add(1);
                }
            }
            _ => {}
        }
        out
    }
}

/// An option declared the old way (`SET_VARIABLES`): `"Description; default|other|..."`.
fn from_legacy(key: String, value: &str) -> Declared {
    let choices = value.split_once("; ").map_or(value, |(_, choices)| choices);
    Declared { key, values: choices.split('|').map(str::to_string).collect(), default: None }
}

/// How tall a frame the core draws with `value` for the option `key`, for the options that say.
fn drawn_height(key: &str, value: &str) -> Option<u32> {
    match key {
        // "1280x960"
        "reicast_internal_resolution" | "flycast_internal_resolution" | "mupen64plus-43screensize" => {
            value.split_once('x')?.1.trim().parse().ok()
        }
        // "2x", or "1x(native)": the PlayStation's 240 lines, so many times over.
        "beetle_psx_hw_internal_resolution" => Some(value.split('x').next()?.trim().parse::<u32>().ok()? * 240),
        _ => None,
    }
}

/// The tallest 3D is drawn, four times the consoles' 240 lines. Every frame is read back from the
/// GPU and scaled on the CPU, so drawing bigger costs time a frame doesn't have: Flycast drawing
/// at 2880×2160, for a panel 2000 pixels tall, took 16 ms over a frame, and the game slowed and its
/// sound broke up. At 960 it's under a millisecond, and the picture at most twice as big again on
/// screen, which is still sharp.
const MAX_DRAWN: u32 = 960;

/// The value of a size option that draws at least `height` tall (or `MAX_DRAWN`, if that's less),
/// as little over as there is, or else the tallest up to `MAX_DRAWN`. None for an option that
/// isn't a size, or offers nothing that small.
fn fitted(d: &Declared, height: u32) -> Option<&String> {
    let height = height.min(MAX_DRAWN);
    let sizes: Vec<(u32, &String)> =
        d.values.iter().filter_map(|v| Some((drawn_height(&d.key, v)?, v))).filter(|&(h, _)| h <= MAX_DRAWN).collect();
    let tall_enough = sizes.iter().filter(|(h, _)| *h >= height).min_by_key(|(h, _)| *h);
    tall_enough.or(sizes.iter().max_by_key(|(h, _)| *h)).map(|(_, v)| *v)
}

/// The options a game is played with.
#[derive(Default)]
pub struct Options {
    /// What's set in RetroArch for the core and the game.
    chosen: BTreeMap<String, String>,
    /// How tall the game is shown, in screen pixels, to draw 3D at about that size.
    shown_height: Option<u32>,
    /// The value given for each key, kept while the core may still hold it.
    values: BTreeMap<String, CString>,
}

impl Options {
    /// Options as `chosen` in RetroArch, for a game shown `shown_height` pixels tall, if 3D is to
    /// be drawn at about that size.
    pub fn new(chosen: BTreeMap<String, String>, shown_height: Option<u32>) -> Options {
        Options { chosen, shown_height, values: BTreeMap::new() }
    }

    /// Takes the options a core declared: each is what's chosen for it, if the core allows that
    /// value, or else its default. A size a 3D core draws at is fitted to how tall it's shown.
    pub fn declare(&mut self, declared: Vec<Declared>) {
        for d in declared {
            let fitted = self.shown_height.and_then(|h| fitted(&d, h));
            if let Some(value) = fitted {
                super::libretro::note(&format!("romi: {} = {value}, to draw about as tall as it's shown", d.key));
            }
            let chosen = self.chosen.get(&d.key).filter(|v| d.values.is_empty() || d.values.contains(v));
            let value = fitted.or(chosen).or(d.default.as_ref()).or(d.values.first());
            if let Some(value) = value.and_then(|v| CString::new(v.as_str()).ok()) {
                self.values.insert(d.key, value);
            }
        }
    }

    /// The value for `key`, as a C string that stays put until the core declares its options
    /// again. A key the core never declared gets what's chosen for it, if anything is.
    pub fn get(&mut self, key: &str) -> Option<*const c_char> {
        if !self.values.contains_key(key) {
            let value = CString::new(self.chosen.get(key)?.as_str()).ok()?;
            self.values.insert(key.to_string(), value);
        }
        self.values.get(key).map(|v| v.as_ptr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(options: &mut Options, key: &str) -> Option<String> {
        options.get(key).map(|p| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }

    #[test]
    fn options_are_what_retroarch_has_if_the_core_allows_it_or_else_the_default() {
        let chosen = [("cpu", "dynamic_recompiler"), ("res", "9999x9999"), ("extra", "on")]
            .map(|(k, v)| (k.to_string(), v.to_string()));
        let mut options = Options::new(chosen.into_iter().collect(), None);
        let declare = |key: &str, values: &[&str], default: Option<&str>| Declared {
            key: key.into(),
            values: values.iter().map(|v| v.to_string()).collect(),
            default: default.map(str::to_string),
        };
        options.declare(vec![
            declare("cpu", &["pure_interpreter", "dynamic_recompiler"], Some("pure_interpreter")),
            declare("res", &["320x240", "640x480"], Some("640x480")),
            declare("audio", &["on", "off"], None),
        ]);
        assert_eq!(value(&mut options, "cpu").as_deref(), Some("dynamic_recompiler"));
        assert_eq!(value(&mut options, "res").as_deref(), Some("640x480"), "not a value the core takes");
        assert_eq!(value(&mut options, "audio").as_deref(), Some("on"), "the first is the default");
        assert_eq!(value(&mut options, "extra").as_deref(), Some("on"), "never declared, but set");
        assert_eq!(value(&mut options, "nothing"), None);
    }

    #[test]
    fn three_d_is_drawn_about_as_tall_as_its_shown() {
        let declare = |key: &str, values: &[&str]| Declared {
            key: key.into(),
            values: values.iter().map(|v| v.to_string()).collect(),
            default: Some(values[0].into()),
        };
        let chosen = [("mupen64plus-43screensize".to_string(), "640x480".to_string())];
        let mut options = Options::new(chosen.into_iter().collect(), Some(910));
        options.declare(vec![
            declare("mupen64plus-43screensize", &["640x480", "320x240", "1280x960", "960x720", "1920x1440"]),
            declare("beetle_psx_hw_internal_resolution", &["1x(native)", "2x", "4x", "8x"]),
            declare("reicast_internal_resolution", &["640x480", "800x600"]),
            declare("mupen64plus-cpucore", &["dynamic_recompiler", "pure_interpreter"]),
        ]);
        assert_eq!(value(&mut options, "mupen64plus-43screensize").as_deref(), Some("1280x960"));
        assert_eq!(value(&mut options, "beetle_psx_hw_internal_resolution").as_deref(), Some("4x"));
        assert_eq!(value(&mut options, "reicast_internal_resolution").as_deref(), Some("800x600"), "the tallest");
        assert_eq!(value(&mut options, "mupen64plus-cpucore").as_deref(), Some("dynamic_recompiler"));
        // A bigger panel gets no more than `MAX_DRAWN`.
        let mut big = Options::new(BTreeMap::new(), Some(2000));
        big.declare(vec![
            declare("reicast_internal_resolution", &["640x480", "1280x960", "1920x1440", "2880x2160"]),
            declare("beetle_psx_hw_internal_resolution", &["1x(native)", "2x", "4x", "8x"]),
        ]);
        assert_eq!(value(&mut big, "reicast_internal_resolution").as_deref(), Some("1280x960"));
        assert_eq!(value(&mut big, "beetle_psx_hw_internal_resolution").as_deref(), Some("4x"));
        let mut not_sized = Options::new(BTreeMap::new(), None);
        not_sized.declare(vec![declare("mupen64plus-43screensize", &["640x480", "1280x960"])]);
        assert_eq!(value(&mut not_sized, "mupen64plus-43screensize").as_deref(), Some("640x480"));
    }

    #[test]
    fn old_style_options_list_their_default_first() {
        let d = from_legacy("k".into(), "Internal resolution; 1x|2x|4x");
        assert_eq!(d.values, ["1x", "2x", "4x"]);
        assert_eq!(d.default, None);
    }
}
