//! Changes made from romi's menus that override the config, like hiding a game. They're kept by
//! game key in `overrides.jsonc` next to the config, which the config includes. romi rewrites
//! that file whole, so it holds nothing else.

use crate::config::Override;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The file romi's menus write to, next to the main config.
pub const OVERRIDES_FILE: &str = "overrides.jsonc";

const HEADER: &str =
    "// Changes made from romi's menus, by game key. romi rewrites this file, so comments here are lost.\n";

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    overrides: BTreeMap<String, Override>,
}

/// Changes the override for `key`, saves it, and makes sure the config includes the file.
/// Overrides left with nothing in them are dropped.
pub fn update(cfg_path: &Path, key: &str, change: impl FnOnce(&mut Override)) -> Result<()> {
    let path = cfg_path.parent().unwrap_or(Path::new(".")).join(OVERRIDES_FILE);
    let mut file: File = match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_reader(json_comments::StripComments::new(raw.as_bytes()))
            .with_context(|| format!("{} has something romi doesn't write in it; fix it first", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let ov = file.overrides.entry(key.to_string()).or_default();
    change(ov);
    if *ov == Override::default() {
        file.overrides.remove(key);
    }
    let text = format!("{HEADER}{}\n", serde_json::to_string_pretty(&file)?);
    let tmp = path.with_extension("jsonc.tmp");
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .with_context(|| format!("writing {}", path.display()))?;

    let Some((raw, own)) = crate::init::existing_config(cfg_path) else {
        bail!("{} can't be read", cfg_path.display());
    };
    let comment = "Changes made from romi's menus, like hidden games.";
    match crate::init::add_include(&raw, &own, OVERRIDES_FILE, comment) {
        Some(edited) if edited == raw => Ok(()),
        Some(edited) => std::fs::write(cfg_path, edited).with_context(|| format!("writing {}", cfg_path.display())),
        None => {
            bail!("couldn't add \"{OVERRIDES_FILE}\" to the \"include\" list in {}; add it by hand", cfg_path.display())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_includes() {
        let dir = std::env::temp_dir().join(format!("romi-overrides-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join("config.json");
        std::fs::write(&cfg_path, "{\n  // mine\n  \"roms_dir\": \"~/ROMs\"\n}\n").unwrap();
        update(&cfg_path, "gba:a.gba", |o| o.hidden = true).unwrap();
        update(&cfg_path, "steam:1", |o| o.hidden = true).unwrap();
        let cfg = crate::config::load(&cfg_path).unwrap();
        assert!(cfg.overrides["gba:a.gba"].hidden && cfg.overrides["steam:1"].hidden);
        assert!(std::fs::read_to_string(&cfg_path).unwrap().contains("// mine"));

        update(&cfg_path, "gba:a.gba", |o| o.hidden = false).unwrap();
        let cfg = crate::config::load(&cfg_path).unwrap();
        assert!(!cfg.overrides.contains_key("gba:a.gba"));
        assert_eq!(cfg.include, [OVERRIDES_FILE]);

        // Something romi didn't write is left alone.
        std::fs::write(dir.join(OVERRIDES_FILE), "{ \"vars\": {} }").unwrap();
        assert!(update(&cfg_path, "gba:a.gba", |o| o.hidden = true).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
