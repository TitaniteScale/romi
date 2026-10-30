//! Core options: the settings a libretro core declares, each with the values it can take and a
//! default, and asks for by key as it runs. romi answers with what's set for the core in RetroArch
//! (see `emucfg::retroarch_core_options`), so a game plays the same in both, or else the core's
//! default.

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

/// The options a game is played with.
#[derive(Default)]
pub struct Options {
    /// What's set in RetroArch for the core and the game.
    chosen: BTreeMap<String, String>,
    /// The value given for each key, kept while the core may still hold it.
    values: BTreeMap<String, CString>,
}

impl Options {
    pub fn new(chosen: BTreeMap<String, String>) -> Options {
        Options { chosen, values: BTreeMap::new() }
    }

    /// Takes the options a core declared: each is what's chosen for it, if the core allows that
    /// value, or else its default.
    pub fn declare(&mut self, declared: Vec<Declared>) {
        for d in declared {
            let chosen = self.chosen.get(&d.key).filter(|v| d.values.is_empty() || d.values.contains(v));
            let value = chosen.or(d.default.as_ref()).or(d.values.first());
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
        let mut options = Options::new(chosen.into_iter().collect());
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
    fn old_style_options_list_their_default_first() {
        let d = from_legacy("k".into(), "Internal resolution; 1x|2x|4x");
        assert_eq!(d.values, ["1x", "2x", "4x"]);
        assert_eq!(d.default, None);
    }
}
