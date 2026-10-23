mod app;
mod art;
mod config;
mod gamepad;
mod launch;
mod library;
mod state;
mod steam;
mod ui;

use anyhow::Result;
use app::{App, TerminalJob};
use gamepad::{PadAction, PadEvent};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, DisableFocusChange, EnableFocusChange, Event, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const HELP: &str = "\
romi - a terminal game launcher

usage: romi [options]

options:
  --check          show what romi finds and anything that looks broken
  --list           print every game as tab-separated system, name, key
  --scrape [id..]  find box art on disk and download the rest (all systems, or just these ids)
  --config <path>  use a different config file (or set ROMI_CONFIG)
  -h, --help       show this help
  -V, --version    show the version

keys:
  j/k ↑/↓   move           h/l ←/→   switch pane      tab/[ ]   next/prev system
  enter     play           /         search           f         favorite
  r         random         d         details pane     ctrl-r    reload config
  q         quit

controller:
  d-pad/stick move   A play   B back   Y favorite   X random
  LB/RB system       LT/RT page        Select details
";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut cfg_path = config::config_path();
    let mut mode = "tui";
    let mut only = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--check" => mode = "check",
            "--list" => mode = "list",
            "--scrape" => mode = "scrape",
            id if mode == "scrape" && !id.starts_with('-') => only.push(id.to_string()),
            "--config" => cfg_path = PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!("--config needs a path"))?),
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("romi {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            other => anyhow::bail!("unknown argument: {other} (try --help)"),
        }
    }

    let cfg = config::load(&cfg_path)?;
    match mode {
        "check" => check(&cfg, &cfg_path),
        "scrape" => art::scrape(&cfg, &library::Library::load(&cfg), &only),
        "list" => {
            let lib = library::Library::load(&cfg);
            let mut out = stdout().lock();
            for g in &lib.games {
                writeln!(out, "{}\t{}\t{}", g.system, g.name, g.key)?;
            }
            Ok(())
        }
        _ => run(cfg, cfg_path),
    }
}

fn check(cfg: &config::Config, cfg_path: &std::path::Path) -> Result<()> {
    let lib = library::Library::load(cfg);
    println!("config  {}", app::tilde(cfg_path));
    println!("roms    {}\n", cfg.roms_dir);
    let roms = config::expand_path(&cfg.roms_dir);
    for sys in &cfg.systems {
        let dir = sys.dir.as_deref().unwrap_or(&sys.id);
        let (mark, detail) = match lib.systems.iter().find(|s| s.id == sys.id) {
            Some(s) => match &s.problem {
                Some(p) => ("!", format!("{:>4} games  {p}", s.games.len())),
                None => ("✓", format!("{:>4} games", s.games.len())),
            },
            None if !roms.join(dir).is_dir() && !config::expand_path(dir).is_dir() => ("-", "no folder".into()),
            None => ("-", "   0 games".into()),
        };
        println!("  {mark} {:<24} {detail}", sys.name);
    }
    for s in &lib.systems {
        if !cfg.systems.iter().any(|c| c.id == s.id) {
            println!("  ✓ {:<24} {:>4} games", s.name, s.games.len());
        }
    }
    if cfg.steam.enabled && steam::find_steam_dir(cfg.steam.dir.as_deref()).is_none() {
        println!("\n  steam is enabled but no Steam install was found");
    }
    println!("\n{} games total", lib.games.len());
    Ok(())
}

fn run(cfg: config::Config, cfg_path: PathBuf) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    if cfg.controller.enabled {
        gamepad::spawn(tx, cfg.controller.swap_confirm);
    }
    let mut app = App::new(cfg, cfg_path);
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableFocusChange)?;
    let result = event_loop(&mut terminal, &mut app, &rx);
    let _ = execute!(stdout(), DisableFocusChange);
    ratatui::restore();
    app.save_selection();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App, rx: &Receiver<PadEvent>) -> Result<()> {
    while !app.quit {
        terminal.draw(|f| ui::draw(f, app))?;
        if event::poll(Duration::from_millis(20))? {
            while event::poll(Duration::ZERO)? {
                app.on_term_event(event::read()?);
            }
        }
        while let Ok(ev) = rx.try_recv() {
            app.on_pad(ev);
        }
        app.tick();
        if let Some(job) = app.terminal_job.take() {
            run_in_terminal(terminal, app, rx, job)?;
        }
    }
    Ok(())
}

/// Hands the terminal to a command, then takes it back.
fn run_in_terminal(terminal: &mut DefaultTerminal, app: &mut App, rx: &Receiver<PadEvent>, job: TerminalJob) -> Result<()> {
    execute!(stdout(), DisableFocusChange, LeaveAlternateScreen)?;
    disable_raw_mode()?;

    let started = Instant::now();
    let status = job.prepared.run_in_terminal();
    let played = started.elapsed();
    let code = match &status {
        Ok(s) => Some(s.code()),
        Err(e) => {
            eprintln!("romi: {e:#}");
            None
        }
    };

    // Leave the output readable until the user is done with it.
    let outcome = match code {
        Some(Some(c)) => format!("exited {c}"),
        Some(None) => "killed".into(),
        None => "failed to start".into(),
    };
    print!("\n\x1b[2m[{outcome}] press any key or A to return to romi\x1b[0m");
    stdout().flush()?;
    enable_raw_mode()?;
    wait_for_any_input(rx);

    execute!(stdout(), EnterAlternateScreen, EnableFocusChange)?;
    terminal.clear()?;
    app.finish(job.game, played, code);
    Ok(())
}

fn wait_for_any_input(rx: &Receiver<PadEvent>) {
    // Drop anything pressed while the command ran.
    while rx.try_recv().is_ok() {}
    loop {
        if let Ok(true) = event::poll(Duration::from_millis(30)) {
            if let Ok(Event::Key(k)) = event::read() {
                if k.kind == KeyEventKind::Press {
                    return;
                }
            }
        }
        if let Ok(PadEvent::Action(PadAction::Confirm | PadAction::Back)) = rx.try_recv() {
            return;
        }
    }
}
