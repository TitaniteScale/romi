//! Commands run when a game starts and when it stops (`hooks` in the config, globally and per
//! system): moving it to a workspace, pausing music, setting the CPU governor. A hook that fails
//! never stops the game; what went wrong is in `hooks.log`, with the hooks' own output.

use crate::config::{Config, state_dir};
use crate::launch::Launch;
use crate::library::Library;
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long romi waits for each `on_launch` hook before starting the game anyway.
const TIMEOUT: Duration = Duration::from_secs(10);

/// One game's hooks, with its placeholders, ready to run.
#[derive(Debug, Clone, Default)]
pub struct Hooks {
    on_launch: Vec<String>,
    on_exit: Vec<String>,
    vars: HashMap<String, String>,
    env: Vec<(String, String)>,
}

pub fn log_path() -> PathBuf {
    state_dir().join("hooks.log")
}

fn open_log(append: bool) -> Option<File> {
    std::fs::create_dir_all(state_dir()).ok()?;
    OpenOptions::new().create(true).write(true).append(append).truncate(!append).open(log_path()).ok()
}

impl Hooks {
    pub fn for_game(cfg: &Config, lib: &Library, g: usize) -> Hooks {
        let game = &lib.games[g];
        let system = lib.system_of(g);
        let own = cfg.systems.iter().find(|s| s.id == system.id).map(|s| &s.hooks);
        let mut on_launch = cfg.hooks.on_launch.list().to_vec();
        let mut on_exit = Vec::new();
        if let Some(own) = own {
            on_launch.extend_from_slice(own.on_launch.list());
            on_exit.extend_from_slice(own.on_exit.list());
        }
        on_exit.extend_from_slice(cfg.hooks.on_exit.list());

        // Global hooks see every kind of game, so the ROM placeholders are always there.
        let mut vars: HashMap<String, String> =
            ["rom", "file", "name", "dir", "appid"].iter().map(|k| (k.to_string(), String::new())).collect();
        vars.extend(game.launch.vars.clone());
        vars.insert("game".into(), game.name.clone());
        vars.insert("key".into(), game.key.clone());
        vars.insert("system".into(), system.id.clone());
        let env = vec![
            ("ROMI_GAME".into(), game.name.clone()),
            ("ROMI_KEY".into(), game.key.clone()),
            ("ROMI_SYSTEM".into(), system.id.clone()),
            ("ROMI_SYSTEM_NAME".into(), system.name.clone()),
            ("ROMI_ROM".into(), vars["rom"].clone()),
        ];
        Hooks { on_launch, on_exit, vars, env }
    }

    /// The commands each hook would run, for `romi play --dry-run`.
    pub fn preview(&self) -> (Vec<Vec<String>>, Vec<Vec<String>>) {
        let exit = self.exit_vars(Some(0), Duration::ZERO);
        let argv = |cmds: &[String], vars: &HashMap<String, String>| {
            cmds.iter()
                .map(|c| prepare(c, vars).map(|(argv, _)| argv).unwrap_or_else(|e| vec![format!("{e:#}")]))
                .collect()
        };
        (argv(&self.on_launch, &self.vars), argv(&self.on_exit, &exit))
    }

    /// Runs the `on_launch` hooks in order, waiting for each. Returns what went wrong, if anything.
    pub fn launch(&self) -> Option<String> {
        if self.on_launch.is_empty() {
            return None;
        }
        // Each launch starts the log afresh, so it's about the game just played.
        let log = open_log(false);
        let mut problems = Vec::new();
        for cmd in &self.on_launch {
            if let Err(e) = run(cmd, &self.vars, &self.env, "launch", log.as_ref()) {
                problems.push(format!("on_launch {e:#}"));
            }
        }
        report(problems)
    }

    /// Runs the `on_exit` hooks in order on a thread of their own, so nothing waits for them
    /// unless it joins the thread. `code` is the game's exit code (128 plus the signal number if
    /// it was killed), or None if that isn't known.
    pub fn exit(&self, code: Option<i32>, played: Duration) -> std::thread::JoinHandle<()> {
        let (cmds, vars, mut env) = (self.on_exit.clone(), self.exit_vars(code, played), self.env.clone());
        env.push(("ROMI_EXIT_CODE".into(), vars["exit_code"].clone()));
        env.push(("ROMI_PLAYED_SECS".into(), vars["played_secs"].clone()));
        std::thread::spawn(move || {
            if cmds.is_empty() {
                return;
            }
            let log = open_log(true);
            for cmd in &cmds {
                let _ = run(cmd, &vars, &env, "exit", log.as_ref());
            }
        })
    }

    fn exit_vars(&self, code: Option<i32>, played: Duration) -> HashMap<String, String> {
        let mut vars = self.vars.clone();
        vars.insert("exit_code".into(), code.map(|c| c.to_string()).unwrap_or_default());
        vars.insert("played_secs".into(), played.as_secs().to_string());
        vars
    }
}

fn prepare(cmd: &str, vars: &HashMap<String, String>) -> Result<(Vec<String>, Option<PathBuf>)> {
    let launch = Launch {
        command: cmd.to_string(),
        mode: Default::default(),
        cwd: None,
        env: HashMap::new(),
        vars: vars.clone(),
    };
    let prepared = launch.prepare()?;
    Ok((prepared.argv, prepared.cwd))
}

/// Runs one hook, waiting up to `TIMEOUT` for it. Its output goes to the log.
fn run(
    cmd: &str,
    vars: &HashMap<String, String>,
    env: &[(String, String)],
    event: &str,
    log: Option<&File>,
) -> Result<()> {
    let note = |line: String| {
        if let Some(mut f) = log {
            let _ = writeln!(f, "{line}");
        }
    };
    let result = (|| {
        let (argv, _) = prepare(cmd, vars)?;
        note(format!("$ {}", shell_words::join(&argv)));
        let output = || log.and_then(|f| f.try_clone().ok()).map_or_else(Stdio::null, Stdio::from);
        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .envs(env.iter().map(|(k, v)| (k, v)))
            .env("ROMI_EVENT", event)
            .stdin(Stdio::null())
            .stdout(output())
            .stderr(output())
            // Its own process group, so ^C meant for the game doesn't reach it.
            .process_group(0)
            .spawn()
            .map_err(|e| anyhow::anyhow!("{}: {e}", argv[0]))?;
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    bail!("`{cmd}` failed ({status})");
                }
                return Ok(());
            }
            if start.elapsed() > TIMEOUT {
                // Left running: it may be meant to (a music player waiting for the game to end).
                bail!("`{cmd}` still running after {}s; not waiting for it", TIMEOUT.as_secs());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    if let Err(e) = &result {
        note(format!("romi: {e:#}"));
    }
    result
}

fn report(problems: Vec<String>) -> Option<String> {
    let first = problems.first()?.clone();
    let more = if problems.len() > 1 { format!(" (and {} more)", problems.len() - 1) } else { String::new() };
    Some(format!("{first}{more}; see {}", crate::config::tilde(&log_path())))
}
