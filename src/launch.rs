//! Turning a command template into a running process.

use crate::config::{LaunchMode, expand_path, expand_tilde};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Everything needed to start a game, with placeholders still unexpanded.
#[derive(Debug, Clone)]
pub struct Launch {
    pub command: String,
    pub mode: LaunchMode,
    pub cwd: Option<String>,
    pub env: HashMap<String, String>,
    /// Placeholder values: config vars plus per-game ones like `rom`.
    pub vars: HashMap<String, String>,
}

pub struct Prepared {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

/// Replaces `{key}` occurrences in `s`. Unknown keys are an error so typos surface early.
pub fn substitute(s: &str, vars: &HashMap<String, String>) -> Result<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return Ok(out);
        };
        let key = &after[..end];
        match vars.get(key) {
            Some(v) => out.push_str(v),
            None => bail!("unknown placeholder {{{key}}}"),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

impl Launch {
    pub fn prepare(&self) -> Result<Prepared> {
        let words =
            shell_words::split(&self.command).with_context(|| format!("bad quoting in command: {}", self.command))?;
        if words.is_empty() {
            bail!("empty command");
        }
        let argv =
            words.iter().map(|w| substitute(w, &self.vars).map(|s| expand_tilde(&s))).collect::<Result<Vec<_>>>()?;
        let cwd = match &self.cwd {
            Some(c) => Some(expand_path(&substitute(c, &self.vars)?)),
            None => None,
        };
        let env = self
            .env
            .iter()
            .map(|(k, v)| Ok((k.clone(), expand_tilde(&substitute(v, &self.vars)?))))
            .collect::<Result<Vec<_>>>()?;
        Ok(Prepared { argv, cwd, env })
    }
}

impl Prepared {
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.argv[0]);
        cmd.args(&self.argv[1..]).envs(self.env.iter().map(|(k, v)| (k, v)));
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        cmd
    }

    /// Starts a game with output going to `log`. With `own_group`, it gets its own process group,
    /// so closing the terminal (or ^C) doesn't take the game down with it; the TUI wants that.
    /// `romi play` doesn't: there, ^C or a script's `timeout` should stop the game too.
    pub fn spawn_logged(&self, log: &Path, own_group: bool) -> Result<Child> {
        if let Some(dir) = log.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let out = std::fs::File::create(log)?;
        let err = out.try_clone()?;
        let mut cmd = self.command();
        cmd.stdin(Stdio::null()).stdout(out).stderr(err);
        if own_group {
            cmd.process_group(0);
        }
        cmd.spawn().map_err(|e| spawn_error(&self.argv[0], e))
    }

    /// Fails if the program isn't installed. Spawning reports that too, but a detached game is
    /// spawned by a background romi whose errors nobody sees, so check first.
    pub fn check_program(&self) -> Result<()> {
        match find_program(&self.argv[0]) {
            Some(_) => Ok(()),
            None => bail!("{}: not found (not installed or not on PATH?)", self.argv[0]),
        }
    }

    /// Starts a game with the terminal attached. The caller must have released the terminal.
    pub fn spawn_attached(&self) -> Result<Child> {
        self.command().spawn().map_err(|e| spawn_error(&self.argv[0], e))
    }
}

fn spawn_error(program: &str, e: std::io::Error) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        anyhow::anyhow!("{program}: not found (not installed or not on PATH?)")
    } else {
        anyhow::anyhow!("{program}: {e}")
    }
}

/// Finds an executable by name on PATH, or checks an explicit path.
pub fn find_program(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let p = PathBuf::from(program);
        return p.is_file().then_some(p);
    }
    std::env::var_os("PATH")?.to_str()?.split(':').map(|dir| Path::new(dir).join(program)).find(|p| p.is_file())
}

/// Sanity-checks a system's command without a real ROM: the program must exist, and so must any
/// absolute paths in it that don't depend on the ROM (like a RetroArch core).
pub fn check_command(command: &str, vars: &HashMap<String, String>, rom_keys: &[&str]) -> Option<String> {
    const MARK: &str = "\u{0}rom\u{0}";
    let mut vars = vars.clone();
    for k in rom_keys {
        vars.insert((*k).to_string(), MARK.to_string());
    }
    let words = match shell_words::split(command) {
        Ok(w) if !w.is_empty() => w,
        Ok(_) => return Some("empty command".into()),
        Err(e) => return Some(format!("bad quoting: {e}")),
    };
    for (i, w) in words.iter().enumerate() {
        let w = match substitute(w, &vars) {
            Ok(w) => expand_tilde(&w),
            Err(e) => return Some(e.to_string()),
        };
        if w.contains(MARK) {
            continue;
        }
        if i == 0 {
            if find_program(&w).is_none() {
                return Some(format!("{w} not found"));
            }
        } else if w.starts_with('/') && !Path::new(&w).exists() {
            let short = Path::new(&w).file_name().map_or(w.clone(), |f| f.to_string_lossy().into());
            return Some(format!("{short} missing"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn substitution_keeps_spaces_in_one_arg() {
        let l = Launch {
            command: "retroarch -L {cores}/mgba_libretro.so {rom}".into(),
            mode: LaunchMode::Wait,
            cwd: None,
            env: HashMap::new(),
            vars: vars(&[("cores", "/c"), ("rom", "/r/Advance Wars (USA).gba")]),
        };
        let p = l.prepare().unwrap();
        assert_eq!(p.argv, ["retroarch", "-L", "/c/mgba_libretro.so", "/r/Advance Wars (USA).gba"]);
    }

    #[test]
    fn unknown_placeholder_errors() {
        assert!(substitute("{nope}", &HashMap::new()).is_err());
        assert_eq!(substitute("a{b", &HashMap::new()).unwrap(), "a{b");
    }

    #[test]
    fn check_skips_rom_args() {
        let v = vars(&[]);
        assert_eq!(check_command("sh -c {rom}", &v, &["rom"]), None);
        assert!(check_command("definitely-not-a-program-xyz {rom}", &v, &["rom"]).is_some());
        assert!(check_command("sh /nonexistent/core.so {rom}", &v, &["rom"]).is_some());
    }
}
