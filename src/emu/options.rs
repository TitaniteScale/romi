//! Core options: the settings a libretro core declares, each with the values it can take and a
//! default, and asks for by key as it runs. romi answers with what's set for the core in RetroArch
//! (see `emucfg::retroarch_core_options`), so a game plays the same in both, or else the core's
//! default.
//!
//! The exception is how big a 3D core draws. Drawn at the console's size (640×480) and scaled
//! up to the panel, a pixel at a time, 3D games look blocky, so romi asks for the size closest to
//! the panel's instead, when it's told how tall that is, up to `MAX_DRAWN`. And a few options have
//! a value that suits romi better (see `in_romi`): melonDS's touchscreen follows the mouse as a
//! pointer, and Dolphin's messages stay off the picture and its CPU on one thread, unless RetroArch
//! has them otherwise.

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
        // "2": the GameCube's 528 lines (its framebuffer's), so many times over.
        "dolphin_efb_scale" => Some(value.trim().parse::<u32>().ok()? * 528),
        // "2x": the PS2's 448 lines (NTSC's, as its games mostly draw), so many times over.
        "play_res_multi" => Some(value.trim().strip_suffix('x')?.parse::<u32>().ok()? * 448),
        // The same, for LRPS2: "1x (Native)", "2x".
        "pcsx2_upscale_multiplier" => Some(value.split('x').next()?.trim().parse::<u32>().ok()? * 448),
        _ => None,
    }
}

/// The tallest 3D is drawn, about four times the consoles' 240 lines. Every frame is read back from
/// the GPU and scaled on the CPU, so drawing bigger costs time a frame doesn't have: Flycast drawing
/// at 2880×2160, for a panel 2000 pixels tall, took 16 ms over a frame, and the game slowed and its
/// sound broke up. At 960 it's under a millisecond, and the picture at most twice as big again on
/// screen, which is still sharp. 1056 lets Dolphin draw at twice its 528 lines (1280×1056, 4 ms a
/// frame headless for Monkey Ball 2, with the reading back); the other cores' next sizes up are
/// 1440 and over.
const MAX_DRAWN: u32 = 1056;

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

/// A value that works better in romi than the one picked, for an option where there's one, or
/// than the core's default (`chosen` says whether it was set in RetroArch). melonDS's "Mouse" touch
/// mode (RetroArch's default) moves a cursor of its own by how far the mouse moves, which drifts
/// from the mouse in romi, where the mouse says where it is. Dolphin's on-screen messages ("Video
/// Info: …" as each game starts) are written into the game's picture, over the game in the panel.
/// Dolphin's dual core (its CPU on a thread of its own) makes the sound out of step with the
/// frames: Kirby Air Ride's came with up to 29 frames in a row without any, and ran dry. On one
/// thread each frame brings one refresh's sound, as romi paces games by, still at 9× full speed.
fn in_romi(d: &Declared, value: &str, chosen: bool) -> Option<&'static str> {
    let better = match (d.key.as_str(), value) {
        ("melonds_touch_mode", "Mouse") => "Touch",
        ("dolphin_osd_enabled", "enabled") if !chosen => "disabled",
        ("dolphin_main_cpu_thread", "enabled") if !chosen => "disabled",
        _ => return None,
    };
    d.values.iter().any(|v| v == better).then_some(better)
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
            let takes = |v: &&String| d.values.is_empty() || d.values.contains(v);
            let chosen = self.chosen.get(&d.key).filter(takes);
            // A default the core doesn't list is its first value, as in RetroArch: Dolphin's shader
            // compilation default is "ShaderCompilationMode::Synchronous", a macro never expanded.
            let default = d.default.as_ref().filter(takes);
            let value = fitted.or(chosen).or(default).or(d.values.first()).map(String::as_str);
            let value = value.map(|v| in_romi(&d, v, chosen.is_some()).unwrap_or(v));
            if let Some(value) = value.and_then(|v| CString::new(v).ok()) {
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
        let mut dolphin = Options::new(BTreeMap::new(), Some(900));
        dolphin.declare(vec![declare("dolphin_efb_scale", &["1", "2", "3", "4", "6"])]);
        assert_eq!(value(&mut dolphin, "dolphin_efb_scale").as_deref(), Some("2"), "1056 lines, 2× its 528");
        let mut play = Options::new(BTreeMap::new(), Some(900));
        play.declare(vec![declare("play_res_multi", &["1x", "2x", "4x", "8x"])]);
        assert_eq!(value(&mut play, "play_res_multi").as_deref(), Some("2x"), "the tallest up to 1056, 896");
        let mut lrps2 = Options::new(BTreeMap::new(), Some(400));
        lrps2.declare(vec![declare("pcsx2_upscale_multiplier", &["1x (Native)", "2x", "4x", "8x"])]);
        assert_eq!(value(&mut lrps2, "pcsx2_upscale_multiplier").as_deref(), Some("1x (Native)"));
        let mut lrps2 = Options::new(BTreeMap::new(), Some(900));
        lrps2.declare(vec![declare("pcsx2_upscale_multiplier", &["1x (Native)", "2x", "4x", "8x"])]);
        assert_eq!(value(&mut lrps2, "pcsx2_upscale_multiplier").as_deref(), Some("2x"));
        let mut small = Options::new(BTreeMap::new(), Some(500));
        small.declare(vec![declare("dolphin_efb_scale", &["1", "2", "3"])]);
        assert_eq!(value(&mut small, "dolphin_efb_scale").as_deref(), Some("1"));
        let mut not_sized = Options::new(BTreeMap::new(), None);
        not_sized.declare(vec![declare("mupen64plus-43screensize", &["640x480", "1280x960"])]);
        assert_eq!(value(&mut not_sized, "mupen64plus-43screensize").as_deref(), Some("640x480"));
    }

    #[test]
    fn a_default_the_core_doesnt_list_is_its_first_value() {
        let mut options = Options::new(BTreeMap::new(), None);
        options.declare(vec![Declared {
            key: "dolphin_shader_compilation_mode".into(),
            values: ["0", "3", "1", "2"].map(String::from).to_vec(),
            default: Some("ShaderCompilationMode::Synchronous".into()),
        }]);
        assert_eq!(value(&mut options, "dolphin_shader_compilation_mode").as_deref(), Some("0"));
    }

    #[test]
    fn melonds_touches_where_the_mouse_is() {
        let touch = |values: &[&str]| Declared {
            key: "melonds_touch_mode".into(),
            values: values.iter().map(|v| v.to_string()).collect(),
            default: None,
        };
        let chosen = |mode: &str| [("melonds_touch_mode".to_string(), mode.to_string())].into_iter().collect();
        let mut options = Options::new(chosen("Mouse"), None);
        options.declare(vec![touch(&["Mouse", "Touch", "Joystick"])]);
        assert_eq!(value(&mut options, "melonds_touch_mode").as_deref(), Some("Touch"));
        let mut default = Options::new(BTreeMap::new(), None);
        default.declare(vec![touch(&["Mouse", "Touch", "Joystick"])]);
        assert_eq!(value(&mut default, "melonds_touch_mode").as_deref(), Some("Touch"));
        let mut stick = Options::new(chosen("Joystick"), None);
        stick.declare(vec![touch(&["Mouse", "Touch", "Joystick"])]);
        assert_eq!(value(&mut stick, "melonds_touch_mode").as_deref(), Some("Joystick"));
        let mut no_touch = Options::new(chosen("Mouse"), None);
        no_touch.declare(vec![touch(&["Mouse", "Joystick"])]);
        assert_eq!(value(&mut no_touch, "melonds_touch_mode").as_deref(), Some("Mouse"));
    }

    #[test]
    fn dolphins_messages_are_off_unless_retroarch_has_them_on() {
        let osd = || Declared {
            key: "dolphin_osd_enabled".into(),
            values: vec!["disabled".into(), "enabled".into()],
            default: Some("enabled".into()),
        };
        let mut default = Options::new(BTreeMap::new(), None);
        default.declare(vec![osd()]);
        assert_eq!(value(&mut default, "dolphin_osd_enabled").as_deref(), Some("disabled"));
        let chosen = [("dolphin_osd_enabled".to_string(), "enabled".to_string())].into_iter().collect();
        let mut on = Options::new(chosen, None);
        on.declare(vec![osd()]);
        assert_eq!(value(&mut on, "dolphin_osd_enabled").as_deref(), Some("enabled"));
    }

    #[test]
    fn dolphins_cpu_is_on_one_thread_unless_retroarch_has_dual_core() {
        let dual = || Declared {
            key: "dolphin_main_cpu_thread".into(),
            values: vec!["disabled".into(), "enabled".into()],
            default: Some("enabled".into()),
        };
        let mut default = Options::new(BTreeMap::new(), None);
        default.declare(vec![dual()]);
        assert_eq!(value(&mut default, "dolphin_main_cpu_thread").as_deref(), Some("disabled"));
        let chosen = [("dolphin_main_cpu_thread".to_string(), "enabled".to_string())].into_iter().collect();
        let mut on = Options::new(chosen, None);
        on.declare(vec![dual()]);
        assert_eq!(value(&mut on, "dolphin_main_cpu_thread").as_deref(), Some("enabled"));
    }

    #[test]
    fn old_style_options_list_their_default_first() {
        let d = from_legacy("k".into(), "Internal resolution; 1x|2x|4x");
        assert_eq!(d.values, ["1x", "2x", "4x"]);
        assert_eq!(d.default, None);
    }
}
