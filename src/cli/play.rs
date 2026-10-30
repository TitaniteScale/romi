//! `romi play`, which launches a game as the TUI does, and `romi run`, which plays one inside romi.

use super::*;

pub(super) fn play(
    cfg: &Config,
    cfg_path: &std::path::Path,
    query: &str,
    system: Option<&str>,
    mode: Option<LaunchMode>,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    // No metadata needed, so this skips the database lookups `list` and `info` do.
    let lib = Library::load(cfg);
    let g = resolve(&lib, query, system)?;
    let game = &lib.games[g];
    let mode = mode.unwrap_or(game.launch.mode);
    let prepared = match game.launch.prepare() {
        Ok(p) => p,
        Err(e) => return fail(exit::CONFIG, format!("{}: {e:#}", game.name)),
    };
    let hooks = crate::hooks::Hooks::for_game(cfg, &lib, g);
    if dry_run {
        return print_command(game, &prepared, mode, &hooks, json);
    }
    if let Err(e) = prepared.check_program() {
        return fail(exit::UNAVAILABLE, format!("{}: {e:#}", game.name));
    }
    let log = app::log_path();
    if mode == LaunchMode::Detach {
        // A copy of romi in wait mode plays it in the background and records the session.
        session::play_in_background(cfg_path, &game.key)?;
        if json {
            println!("{}", serde_json::json!({ "key": game.key, "name": game.name, "mode": mode }));
        }
        return Ok(());
    }
    catch_stop_signals();
    if let Some(problem) = hooks.launch() {
        eprintln!("romi: {problem}");
    }
    let mut child = match mode {
        LaunchMode::Terminal => prepared.spawn_attached()?,
        _ => prepared.spawn_logged(&log, false)?,
    };
    let (session, _) = Session::begin(&lib, g, child.id(), hooks);
    let status = wait_for_game(&mut child)?;
    let (played, _, exit_hooks) = session.end(Some(status));
    let code = session::exit_code(status);
    // romi is about to exit, which would cut the hooks short.
    let _ = exit_hooks.join();
    let log = (mode == LaunchMode::Wait).then(|| log.to_string_lossy().into_owned());
    if json {
        let outcome = serde_json::json!({
            "key": game.key,
            "name": game.name,
            "mode": mode,
            "exit_code": code,
            "played_secs": played.as_secs(),
            "log": log,
        });
        println!("{outcome}");
    }
    if code != 0 {
        let how = match status.signal() {
            Some(sig) => format!("was killed by signal {sig}"),
            None => format!("exited with code {code}"),
        };
        let log = log.map(|l| format!(" (log: {})", config::tilde(std::path::Path::new(&l)))).unwrap_or_default();
        return fail(code, format!("{} {how}{log}", game.name));
    }
    Ok(())
}

fn print_command(game: &Game, prepared: &Prepared, mode: LaunchMode, hooks: &Hooks, json: bool) -> Result<()> {
    let cwd = prepared.cwd.as_ref().map(|c| c.to_string_lossy().into_owned());
    let (on_launch, on_exit) = hooks.preview();
    if json {
        let env: serde_json::Map<String, serde_json::Value> =
            prepared.env.iter().map(|(k, v)| (k.clone(), v.clone().into())).collect();
        let command = serde_json::json!({
            "key": game.key,
            "name": game.name,
            "mode": mode,
            "argv": prepared.argv,
            "cwd": cwd,
            "env": env,
            "hooks": { "on_launch": on_launch, "on_exit": on_exit },
        });
        println!("{}", serde_json::to_string_pretty(&command)?);
        return Ok(());
    }
    println!("mode     {}", serde_json::to_value(mode)?.as_str().unwrap_or_default());
    println!("command  {}", shell_words::join(&prepared.argv));
    if let Some(cwd) = cwd {
        println!("cwd      {cwd}");
    }
    for (k, v) in &prepared.env {
        println!("env      {k}={}", shell_words::quote(v));
    }
    // On exit, {exit_code} is shown as 0 and {played_secs} as 0.
    for (label, argv) in on_launch.iter().map(|a| ("on_launch", a)).chain(on_exit.iter().map(|a| ("on_exit  ", a))) {
        println!("{label} {}", shell_words::join(argv));
    }
    Ok(())
}

/// The signal that asked romi to stop while a game was running, or 0.
static STOP_SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_stop_signal(sig: libc::c_int) {
    STOP_SIGNAL.store(sig, Ordering::Relaxed);
}

/// Keeps romi alive long enough to record the session when it's told to stop. Must run before
/// the game starts, so there's no moment when a signal would kill romi and leave the game going.
fn catch_stop_signals() {
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: the handler only stores to an atomic. A handler, unlike SIG_IGN, is reset to the
        // default when the game's program is exec'd, so the game still reacts to signals normally.
        unsafe { libc::signal(sig, on_stop_signal as extern "C" fn(libc::c_int) as libc::sighandler_t) };
    }
}

/// Waits for the game to exit. ^C in the terminal, or `timeout` in a script, signals the whole
/// process group, which the game shares with romi, so the game gets it too and romi just waits.
/// A signal sent to romi alone is passed on if the game is still running a second later.
fn wait_for_game(child: &mut Child) -> std::io::Result<ExitStatus> {
    const PASS_ON_AFTER: Duration = Duration::from_secs(1);
    let mut signalled: Option<(i32, Instant)> = None;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        let sig = STOP_SIGNAL.swap(0, Ordering::Relaxed);
        if sig != 0 {
            signalled = Some((sig, Instant::now()));
        }
        if let Some((sig, at)) = signalled
            && at.elapsed() >= PASS_ON_AFTER
        {
            // SAFETY: kill(2) on our own child's pid; it can't be reused before we reap it.
            unsafe { libc::kill(child.id() as libc::pid_t, sig) };
            signalled = None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---- run ----

#[allow(clippy::too_many_arguments)]
pub(super) fn run_core(
    cfg: &Config,
    query: &str,
    system: Option<&str>,
    core: Option<PathBuf>,
    frames: u64,
    screenshot: Option<&Path>,
    wav: Option<&Path>,
    input: Option<&Path>,
    json: bool,
    live: bool,
) -> Result<()> {
    let (name, rom, launch) = match Path::new(query).is_file() {
        true => rom_file(cfg, Path::new(query), system)?,
        false => {
            let lib = Library::load(cfg);
            let game = &lib.games[resolve(&lib, query, system)?];
            let Some(rom) = game.path.clone() else {
                return fail(exit::USAGE, format!("{} isn't a ROM", game.name));
            };
            (game.name.clone(), rom, Some(game.launch.clone()))
        }
    };
    let core = match core {
        Some(c) => c,
        None => {
            let argv = match launch.as_ref().map(|l| l.prepare()) {
                Some(Ok(p)) => p.argv,
                Some(Err(e)) => return fail(exit::CONFIG, format!("{name}: {e:#}")),
                None => Vec::new(),
            };
            match crate::emu::libretro::core_in(&argv) {
                Some(c) => c,
                None => return fail(exit::USAGE, format!("{name} doesn't run through a libretro core; pass --core")),
            }
        }
    };
    if !core.is_file() {
        return fail(exit::UNAVAILABLE, format!("no core at {}", config::tilde(&core)));
    }
    if !crate::emu::libretro::can_open(&core, &rom) {
        let why = "this core can't open 7z files, and romi can only unpack zips; extract it first";
        return fail(exit::UNAVAILABLE, format!("{}: {why}", rom.display()));
    }
    let script = match input {
        Some(path) => {
            let text = std::fs::read_to_string(path).with_context(|| format!("can't read {}", path.display()))?;
            crate::emu::libretro::Script::parse(&text)
                .or_else(|e| fail(exit::USAGE, format!("{}: {e:#}", path.display())))?
        }
        None => crate::emu::libretro::Script::default(),
    };

    if live {
        let emu = crate::emu::runner::load(cfg, &core, &rom)?;
        return crate::emu::live::play(emu, &name, cfg.volume_gain());
    }
    // Runs start with nothing saved, so they repeat exactly, and leave nothing behind. Declared
    // before the core, so it's removed after the core has shut down and written its last.
    let saves = crate::files::TempDir::new("saves").context("can't make a save folder in /tmp")?;
    let dirs = crate::emu::libretro::Dirs {
        system: cfg.system_dir(),
        saves: Some(saves.path().to_path_buf()),
        save_file: None,
        // The same options as playing it, so a run shows what playing would.
        options: crate::emu::libretro::info(&core)
            .map(|info| crate::emucfg::retroarch_core_options(&info.name, &rom))
            .unwrap_or_default(),
    };
    let mut emu = crate::emu::libretro::Core::load(&core, &rom, &dirs)?;
    let mut sound = wav.map(|path| crate::emu::audio::Wav::create(path, emu.sample_rate)).transpose()?;
    let mut samples = Vec::new();
    let started = Instant::now();
    for n in 0..frames {
        emu.run_frame(crate::emu::libretro::Input::buttons(script.buttons_at(n)));
        if let Some(sound) = &mut sound {
            samples.clear();
            emu.audio(&mut samples);
            sound.write(&samples)?;
        }
    }
    let elapsed = started.elapsed();
    let sound_len = sound.map(|s| s.finish()).transpose()?;
    let frame = emu.frame();
    if let Some(path) = screenshot {
        let Some(img) = image::RgbImage::from_raw(frame.width, frame.height, frame.rgb) else {
            bail!("{} hasn't drawn anything after {frames} frames", emu.name);
        };
        img.save(path).with_context(|| format!("can't save {}", path.display()))?;
    }
    let speed = frames as f64 / elapsed.as_secs_f64().max(1e-9);
    if json {
        let outcome = serde_json::json!({
            "name": name,
            "rom": rom,
            "core": core,
            "core_name": emu.name,
            "frames": frames,
            "width": frame.width,
            "height": frame.height,
            "pixel_aspect": if frame.pixel_aspect > 0.0 { frame.pixel_aspect } else { 1.0 },
            "fps": emu.fps,
            "elapsed_secs": elapsed.as_secs_f64(),
            "screenshot": screenshot,
            "sample_rate": emu.sample_rate,
            "wav": wav,
            "wav_secs": sound_len.map(|d| d.as_secs_f64()),
        });
        println!("{outcome}");
    } else {
        println!(
            "Ran {frames} frames of {name} in {:.2}s with {} ({speed:.0} fps, {:.1}x full speed)",
            elapsed.as_secs_f64(),
            emu.name,
            speed / emu.fps,
        );
        if let Some(path) = screenshot {
            println!("Saved the last frame ({}×{}) to {}", frame.width, frame.height, path.display());
        }
        if let (Some(path), Some(len)) = (wav, sound_len) {
            let rate = emu.sample_rate.round();
            println!("Saved {:.1}s of sound ({rate} Hz) to {}", len.as_secs_f64(), path.display());
        }
    }
    Ok(())
}

/// A ROM given by path runs with the command of the first system that lists its extension.
fn rom_file(
    cfg: &Config,
    rom: &Path,
    system: Option<&str>,
) -> Result<(String, PathBuf, Option<crate::launch::Launch>)> {
    let ext = rom.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let sys = match system {
        Some(want) => cfg
            .systems
            .iter()
            .find(|s| s.id.eq_ignore_ascii_case(want) || cfg.system_name(s).eq_ignore_ascii_case(want)),
        None => cfg.system_for_extension(&ext),
    };
    let name = rom.file_stem().unwrap_or(rom.as_os_str()).to_string_lossy().into_owned();
    let launch = sys.map(|s| {
        let mut vars = cfg.vars.clone();
        let rom_str = rom.to_string_lossy().into_owned();
        vars.insert("rom".into(), rom_str);
        vars.insert("file".into(), rom.file_name().unwrap_or_default().to_string_lossy().into_owned());
        vars.insert("name".into(), name.clone());
        vars.insert("dir".into(), rom.parent().unwrap_or(Path::new(".")).to_string_lossy().into_owned());
        crate::launch::Launch { command: s.command.clone(), mode: s.mode, cwd: None, env: s.env.clone(), vars }
    });
    Ok((name, rom.to_path_buf(), launch))
}
