mod add;
mod app;
mod art;
mod cheevos;
mod cli;
mod config;
mod cover;
mod embedded;
mod emucfg;
mod emulators;
mod filter;
mod gamepad;
mod hooks;
mod init;
mod kitty;
mod launch;
mod library;
mod live;
mod meta;
mod overrides;
mod ra;
mod retro;
mod session;
mod sort;
mod state;
mod stats;
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
use session::Session;
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

fn main() {
    cli::complete();
    let (command, cfg_path) = cli::parse();
    let result = match command {
        Some(command) => cli::run(command, &cfg_path),
        None => cli::first_run(&cfg_path).and_then(|ready| match ready {
            true => cli::load_config(&cfg_path).and_then(|cfg| run(cfg, cfg_path)),
            false => Ok(()),
        }),
    };
    if let Err(e) = result {
        // Output piped into something that stopped reading (`romi list | head`) isn't a problem.
        if e.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe) {
            std::process::exit(0);
        }
        let code = match e.downcast_ref::<cli::Exit>() {
            Some(exit) => exit.code,
            None => 1,
        };
        eprintln!("romi: {e:#}");
        std::process::exit(code);
    }
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
        app.pump_embedded();
        // A game in the middle panel needs a new frame every 16ms.
        let wait = if app.embedded.is_some() { Duration::from_millis(4) } else { Duration::from_millis(20) };
        if event::poll(wait)? {
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

    let mut played = Duration::ZERO;
    let hooks = hooks::Hooks::for_game(&app.cfg, &app.lib, job.game);
    if let Some(problem) = hooks.launch() {
        eprintln!("romi: {problem}");
    }
    let code = match job.prepared.spawn_attached() {
        Ok(mut child) => {
            let (session, state) = Session::begin(&app.lib, job.game, child.id(), hooks);
            app.state = state;
            let status = child.wait();
            let (time, state, _) = session.end(status.as_ref().ok().copied());
            (app.state, played) = (state, time);
            status.ok().map(|s| s.code())
        }
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
