mod app;
mod art;
mod config;
mod cover;
mod filter;
mod gamepad;
mod launch;
mod library;
mod meta;
mod state;
mod steam;
mod titles;
mod ui;

use anyhow::Result;
use app::{App, TerminalJob};
use gamepad::{PadAction, PadEvent};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, DisableFocusChange, EnableFocusChange, Event, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
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
  r         random         d         details pane     m         menu
  F         filter         ctrl-r    reload config    q         quit

controller:
  d-pad/stick move (hold to speed up)   A play   B back   Y favorite   X random
  LB/RB system   LT/RT previous/next letter   Select search   Start menu (incl. quit)
  R3 (click right stick) filter by genre, players, decade, played
  searching: d-pad picks a key, A types, X deletes, Y space, B done
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
            "--config" => {
                cfg_path = PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!("--config needs a path"))?)
            }
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
        "scrape" => art::scrape(&cfg, &library::Library::load_complete(&cfg), &only),
        "list" => {
            let lib = library::Library::load_complete(&cfg);
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
    let lib = library::Library::load_complete(cfg);
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
    let count = |f: &dyn Fn(&library::Game) -> bool| lib.games.iter().filter(|g| f(g)).count();
    println!(
        "\nmetadata  {} genre · {} year · {} players · {} description",
        count(&|g| !g.meta.genres.is_empty()),
        count(&|g| g.meta.year.is_some()),
        count(&|g| g.meta.players.is_some()),
        count(&|g| g.meta.description.is_some()),
    );
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
    // Ask the terminal what image protocol it speaks; this has to happen before reading events.
    if app.cfg.art.show {
        let picker = if terminal_answers() {
            ratatui_image::picker::Picker::from_query_stdio().ok()
        } else {
            Some(ratatui_image::picker::Picker::halfblocks())
        };
        app.covers = picker.map(|p| cover::Covers::new(p, &app.cfg));
    }
    // Terminals that don't understand the query may have echoed part of it. Nothing is drawn yet,
    // so wiping the screen keeps it in sync with ratatui's (empty) buffer.
    execute!(stdout(), Clear(ClearType::All))?;
    execute!(stdout(), EnableFocusChange)?;
    let result = event_loop(&mut terminal, &mut app, &rx);
    let _ = execute!(stdout(), DisableFocusChange);
    ratatui::restore();
    app.save_selection();
    result
}

/// Asks the terminal for a status report and waits for the reply. The image-support query reads
/// stdin on a thread that outlives its own timeout, so on a terminal that never answers, that
/// thread would swallow the first keypress. Asking first means the query only runs if it can finish.
fn terminal_answers() -> bool {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let mut out = stdout();
    if out.write_all(b"\x1b[5n").and_then(|()| out.flush()).is_err() {
        return false;
    }
    let stdin = std::io::stdin();
    let deadline = Instant::now() + Duration::from_secs(1);
    // The reply is `ESC [ 0 n`. Read it a byte at a time, straight from the fd, so nothing after
    // it is taken from the event reader.
    let mut byte = [0u8];
    loop {
        let Ok(left) = Timespec::try_from(deadline.saturating_duration_since(Instant::now())) else {
            return false;
        };
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        match poll(&mut fds, Some(&left)) {
            Ok(n) if n > 0 => {}
            _ => return false,
        }
        match rustix::io::read(&stdin, &mut byte) {
            Ok(1) if byte[0] == b'n' => return true,
            Ok(1) => {}
            _ => return false,
        }
    }
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
fn run_in_terminal(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    rx: &Receiver<PadEvent>,
    job: TerminalJob,
) -> Result<()> {
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
    if let Some(c) = &mut app.covers {
        c.forget_rendered();
    }
    app.finish(job.game, played, code);
    Ok(())
}

fn wait_for_any_input(rx: &Receiver<PadEvent>) {
    // Drop anything pressed while the command ran.
    while rx.try_recv().is_ok() {}
    loop {
        if let Ok(true) = event::poll(Duration::from_millis(30))
            && let Ok(Event::Key(k)) = event::read()
            && k.kind == KeyEventKind::Press
        {
            return;
        }
        if let Ok(PadEvent::Action(PadAction::Confirm | PadAction::Back | PadAction::Menu)) = rx.try_recv() {
            return;
        }
    }
}
