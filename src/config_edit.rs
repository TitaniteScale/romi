//! Changing one setting in the config as you wrote it: only its value's text changes, so comments,
//! order and spacing stay as they were. romi's options menu saves through this.
//!
//! The text is walked as JSON with comments, so a key is found by where it is (`"enabled"` in
//! `"controller"`, not in `"steam"`), never inside a comment or a string. Every edit is checked by
//! parsing the result: it has to come out as the old config with just that value changed.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::Path;

/// Sets the setting at `path` (like `["launch_animation", "enabled"]`) to `value` in the config
/// file, adding it, and objects on the way to it, if they're missing.
pub fn save(cfg_path: &Path, path: &[&str], value: &Value) -> Result<()> {
    let raw = std::fs::read_to_string(cfg_path).with_context(|| format!("reading {}", cfg_path.display()))?;
    let Some(edited) = set(&raw, path, value) else {
        bail!("couldn't change \"{}\" in {}; change it by hand", path.join("."), cfg_path.display());
    };
    crate::files::write_atomic(cfg_path, edited).with_context(|| format!("writing {}", cfg_path.display()))
}

/// The config text with the value at `path` set to `value`, or None if that can't be done cleanly
/// (the text doesn't parse, or something on the way isn't an object).
pub fn set(raw: &str, path: &[&str], value: &Value) -> Option<String> {
    let (&last, parents) = path.split_last()?;
    let mut object = Object::read(raw, open_brace(raw, 0)?)?;
    let mut found = 0;
    for &key in parents {
        let Some(member) = object.member(key) else { break };
        if raw.as_bytes()[member.start] != b'{' {
            return None;
        }
        object = Object::read(raw, member.start)?;
        found += 1;
    }
    let missing = &path[found..];
    let edited = match object.member(last) {
        Some(member) if missing.len() == 1 => {
            format!("{}{}{}", &raw[..member.start], serde_json::to_string(value).ok()?, &raw[member.end..])
        }
        _ => {
            // The first missing key, holding the rest of the path down to the value.
            let mut text = serde_json::to_string(value).ok()?;
            for key in missing[1..].iter().rev() {
                text = format!("{{ {}: {text} }}", serde_json::to_string(key).ok()?);
            }
            let text = format!("{}: {text}", serde_json::to_string(missing[0]).ok()?);
            object.insert(raw, &text)
        }
    };
    // It has to read back as the old config with just this value changed.
    let mut want = parse(raw)?;
    let mut at = &mut want;
    for key in path {
        at = at.as_object_mut()?.entry(key.to_string()).or_insert_with(|| Value::Object(Default::default()));
    }
    *at = value.clone();
    (parse(&edited)? == want).then_some(edited)
}

fn parse(raw: &str) -> Option<Value> {
    serde_json::from_reader(json_comments::StripComments::new(raw.as_bytes())).ok()
}

/// An object in the text: where it opens and closes, and its members.
struct Object {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

/// A key and where its value's text starts and ends.
struct Member {
    key: String,
    /// Where the key's opening quote is.
    at: usize,
    start: usize,
    end: usize,
}

impl Object {
    /// Reads the object whose `{` is at `open`.
    fn read(raw: &str, open: usize) -> Option<Object> {
        let s = raw.as_bytes();
        let mut members = Vec::new();
        let mut i = skip(s, open + 1);
        loop {
            match *s.get(i)? {
                b'}' => return Some(Object { open, close: i, members }),
                b'"' => {
                    let key_end = string_end(s, i)?;
                    let key: String = serde_json::from_str(&raw[i..key_end]).ok()?;
                    let colon = skip(s, key_end);
                    if s.get(colon) != Some(&b':') {
                        return None;
                    }
                    let start = skip(s, colon + 1);
                    let end = value_end(s, start)?;
                    members.push(Member { key, at: i, start, end });
                    i = skip(s, end);
                    if s.get(i) == Some(&b',') {
                        i = skip(s, i + 1);
                    }
                }
                _ => return None,
            }
        }
    }

    /// The last member with this key, which is the one that counts.
    fn member(&self, key: &str) -> Option<&Member> {
        self.members.iter().rev().find(|m| m.key == key)
    }

    /// The text with `member` (`"key": value`) added as this object's first member, laid out like
    /// the others: on a line of its own if they are.
    fn insert(&self, raw: &str, member: &str) -> String {
        let after = self.open + 1;
        let Some(first) = self.members.first() else {
            return format!("{} {member} {}", &raw[..after], &raw[self.close..]);
        };
        let line_start = raw[..first.at].rfind('\n').map_or(0, |n| n + 1);
        let indent = &raw[line_start..first.at];
        if line_start > self.open && indent.trim().is_empty() {
            format!("{}\n{indent}{member},{}", &raw[..after], &raw[after..])
        } else {
            format!("{} {member},{}", &raw[..after], &raw[after..])
        }
    }
}

/// The first `{` at or after `from`, outside comments.
fn open_brace(raw: &str, from: usize) -> Option<usize> {
    let i = skip(raw.as_bytes(), from);
    (raw.as_bytes().get(i) == Some(&b'{')).then_some(i)
}

/// Past whitespace and comments.
fn skip(s: &[u8], mut i: usize) -> usize {
    loop {
        match (s.get(i), s.get(i + 1)) {
            (Some(c), _) if c.is_ascii_whitespace() => i += 1,
            (Some(b'/'), Some(b'/')) => {
                i = s[i..].iter().position(|&c| c == b'\n').map_or(s.len(), |n| i + n);
            }
            (Some(b'/'), Some(b'*')) => {
                i = s[i + 2..].windows(2).position(|w| w == b"*/").map_or(s.len(), |n| i + 2 + n + 2);
            }
            _ => return i,
        }
    }
}

/// Just past the string whose opening quote is at `i`.
fn string_end(s: &[u8], mut i: usize) -> Option<usize> {
    i += 1;
    loop {
        match *s.get(i)? {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
}

/// Just past the value that starts at `i`: a string, an object or array (with whatever is nested
/// in it), or a number, `true`, `false` or `null`.
fn value_end(s: &[u8], i: usize) -> Option<usize> {
    match *s.get(i)? {
        b'"' => string_end(s, i),
        b'{' | b'[' => {
            let mut depth = 0;
            let mut j = i;
            loop {
                j = skip(s, j);
                match *s.get(j)? {
                    b'"' => j = string_end(s, j)?,
                    b'{' | b'[' => {
                        depth += 1;
                        j += 1;
                    }
                    b'}' | b']' => {
                        depth -= 1;
                        j += 1;
                        if depth == 0 {
                            return Some(j);
                        }
                    }
                    _ => j += 1,
                }
            }
        }
        _ => {
            let n = s[i..]
                .iter()
                .position(|&c| c == b',' || c == b'}' || c == b']' || c == b'/' || c.is_ascii_whitespace());
            Some(i + n.unwrap_or(s.len() - i)).filter(|&end| end > i)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CONFIG: &str = r#"{
  // How loud: "volume": 0 would mute.
  "volume": 100,
  "controller": {
    "enabled": true, /* the pad */
    "swap_confirm": false
  },
  "steam": { "enabled": true },
  "systems": [{ "id": "gb", "command": "x {rom}" }]
}
"#;

    #[test]
    fn changes_only_the_value() {
        let edited = set(CONFIG, &["volume"], &json!(40)).unwrap();
        assert_eq!(edited, CONFIG.replace("\"volume\": 100", "\"volume\": 40"));
        // Not the one in the comment, and not steam's "enabled".
        let edited = set(CONFIG, &["controller", "enabled"], &json!(false)).unwrap();
        assert_eq!(edited, CONFIG.replace("\"enabled\": true, /*", "\"enabled\": false, /*"));
        let edited = set(CONFIG, &["steam", "enabled"], &json!(false)).unwrap();
        assert_eq!(edited, CONFIG.replace("{ \"enabled\": true }", "{ \"enabled\": false }"));
    }

    #[test]
    fn adds_what_is_missing_laid_out_like_the_rest() {
        let edited = set(CONFIG, &["corner_radius"], &json!(4)).unwrap();
        assert!(edited.starts_with("{\n  \"corner_radius\": 4,\n  // How loud"), "{edited}");
        let edited = set(CONFIG, &["launch_animation", "enabled"], &json!(false)).unwrap();
        assert!(edited.contains("\n  \"launch_animation\": { \"enabled\": false },\n"), "{edited}");
        let edited = set(CONFIG, &["steam", "group"], &json!("PC")).unwrap();
        assert!(edited.contains("\"steam\": { \"group\": \"PC\", \"enabled\": true }"), "{edited}");
        assert_eq!(set("{}", &["volume"], &json!(5)).unwrap(), "{ \"volume\": 5 }");
    }

    #[test]
    fn refuses_what_it_cant_do_cleanly() {
        assert_eq!(set(CONFIG, &["volume", "x"], &json!(1)), None, "volume isn't an object");
        assert_eq!(set("{ \"volume\": 1, }", &["volume"], &json!(2)), None, "doesn't parse");
        assert_eq!(set("[]", &["volume"], &json!(2)), None);
    }
}
