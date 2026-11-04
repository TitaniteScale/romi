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

/// What a `romi run` is asked to do.
pub(super) struct Run<'a> {
    pub frames: u64,
    pub screenshot: Option<&'a Path>,
    pub wav: Option<&'a Path>,
    pub input: Option<&'a Path>,
    pub load_state: Option<&'a Path>,
    pub save_state: Option<&'a Path>,
    pub achievements: bool,
    pub json: bool,
    pub live: bool,
}

pub(super) fn run_core(cfg: &Config, query: &str, system: Option<&str>, core: Option<PathBuf>, run: Run) -> Result<()> {
    let Run { frames, screenshot, wav, input, load_state, save_state, achievements, json, live } = run;
    // A game in the library plays with the buttons remapped in romi for it, and with the core
    // from another of its emulators if its own isn't one.
    let (name, rom, launch, controls, core_options, other_core, system_id) = match Path::new(query).is_file() {
        true => {
            let (name, rom, launch, system_id) = rom_file(cfg, Path::new(query), system)?;
            let core_options = system_id.iter().filter_map(|id| cfg.core_options.get(id)).cloned().collect();
            (name, rom, launch, Vec::new(), core_options, None, system_id)
        }
        false => {
            let lib = Library::load(cfg);
            let g = resolve(&lib, query, system)?;
            let game = &lib.games[g];
            let Some(rom) = game.path.clone() else {
                return fail(exit::USAGE, format!("{} isn't a ROM", game.name));
            };
            let controls = crate::emu::runner::controls_for(cfg, &lib, g).into_iter().cloned().collect();
            let core_options: Vec<_> =
                crate::emu::runner::core_options_for(cfg, &lib, g).into_iter().cloned().collect();
            let mut emulators = crate::emulators::Emulators::default();
            emulators.find(cfg, &lib, g);
            let other_core = crate::emulators::core(game, &emulators.options(&lib, g));
            let system_id = Some(lib.system_of(g).id.clone());
            (game.name.clone(), rom, Some(game.launch.clone()), controls, core_options, other_core, system_id)
        }
    };
    let controls: Vec<_> = controls.iter().collect();
    let core_options: Vec<_> = core_options.iter().collect();
    let core = match core {
        Some(c) => c,
        None => {
            let argv = match launch.as_ref().map(|l| l.prepare()) {
                Some(Ok(p)) => p.argv,
                Some(Err(e)) => return fail(exit::CONFIG, format!("{name}: {e:#}")),
                None => Vec::new(),
            };
            // Its own core even when it isn't there, if there's no other, to say so.
            let own = crate::emu::libretro::core_in(&argv);
            match own.clone().filter(|c| c.is_file()).or(other_core).or(own) {
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
        // The whole terminal, less the status line.
        let shown = ratatui::crossterm::terminal::window_size().ok().map(|s| {
            let (_, cell_h) = crate::emu::kitty::cell_size(&s);
            u32::from(s.rows.saturating_sub(1)) * cell_h
        });
        let runs_with = launch.as_ref().map(|l| l.command.as_str());
        let emu = crate::emu::runner::load(cfg, &core, &rom, runs_with, &controls, &core_options, shown)?;
        return crate::emu::live::play(emu, &name, cfg.volume_gain());
    }
    // Runs start with nothing saved, so they repeat exactly, and leave nothing behind. Declared
    // before the core, so it's removed after the core has shut down and written its last.
    let saves = crate::files::TempDir::new("saves").context("can't make a save folder in /tmp")?;
    // The same options and remap as playing it, so a run shows what playing would, at the
    // console's size, so its screenshots are the same whatever the terminal.
    let core_name = crate::emu::libretro::info(&core).map(|info| info.name);
    let dirs = crate::emu::libretro::Dirs {
        system: cfg.system_dir(),
        saves: Some(saves.path().to_path_buf()),
        save_file: None,
        options: crate::emu::runner::options(core_name.as_deref(), &rom, &core_options),
        remap: crate::emu::runner::remap(core_name.as_deref(), &rom, &controls),
        shown_height: None,
    };
    let mut cheevos = achievements.then(|| start_achievements(system_id.as_deref(), &rom)).transpose()?;
    let mut emu = crate::emu::libretro::Core::load(&core, &rom, &dirs)?;
    let mut sound = wav.map(|path| crate::emu::audio::Wav::create(path, emu.sample_rate)).transpose()?;
    let mut samples = Vec::new();
    let mut earned = Vec::new();
    let mut waited = Duration::ZERO;
    let loaded = load_state.map(|path| start_from_state(&mut emu, path)).transpose()?;
    let started = Instant::now();
    for n in 0..frames {
        emu.run_frame(script.players_at(n));
        if let Some(client) = &mut cheevos {
            client.frame(&emu);
            earned.extend(client.take_events());
            if n == 0 {
                waited = wait_for_achievements(client, &mut earned);
            }
        }
        if let Some(sound) = &mut sound {
            samples.clear();
            emu.audio(&mut samples);
            sound.write(&samples)?;
        }
    }
    let elapsed = started.elapsed().saturating_sub(waited);
    if let Some(client) = &mut cheevos {
        client.idle();
        earned.extend(client.take_events());
    }
    let sound_len = sound.map(|s| s.finish()).transpose()?;
    emu.finish_frame();
    let frame = emu.frame();
    let saved = save_state.map(|path| save_to_state(&mut emu, path)).transpose()?;
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
            "load_state_secs": loaded.map(|d| d.as_secs_f64()),
            "save_state": save_state,
            "save_state_secs": saved.map(|(took, _)| took.as_secs_f64()),
            "save_state_bytes": saved.map(|(_, bytes)| bytes),
            "achievements": cheevos.is_some().then(|| achievements_json(&earned)),
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
        if let (Some(path), Some((took, bytes))) = (save_state, saved) {
            let (mb, ms) = (bytes as f64 / 1e6, took.as_secs_f64() * 1e3);
            println!("Saved the state ({mb:.1} MB, in {ms:.0} ms) to {}", path.display());
        }
        if cheevos.is_some() {
            for line in achievements_lines(&earned) {
                println!("{line}");
            }
        }
    }
    Ok(())
}

/// Loads the state in the file at `path`, once the core can take one, and says how long the core
/// took over it.
fn start_from_state(emu: &mut crate::emu::libretro::Core, path: &Path) -> Result<Duration> {
    let file = std::fs::read(path).with_context(|| format!("can't read {}", path.display()))?;
    let state = crate::emu::savestate::decode(file).with_context(|| format!("can't load {}", path.display()))?;
    for _ in 0..crate::emu::libretro::FRAMES_BEFORE_STATES {
        emu.run_frame(Default::default());
    }
    let started = Instant::now();
    emu.load_state(&state.core).with_context(|| format!("can't load {}", path.display()))?;
    Ok(started.elapsed())
}

/// Saves a state to the file at `path`, and says how long the core took over it and how big it is.
fn save_to_state(emu: &mut crate::emu::libretro::Core, path: &Path) -> Result<(Duration, usize)> {
    let started = Instant::now();
    let core = emu.save_state()?;
    let took = started.elapsed();
    let file = crate::emu::savestate::encode(&crate::emu::savestate::State { core, achievements: None })?;
    crate::files::write_atomic(path, &file).with_context(|| format!("can't write {}", path.display()))?;
    Ok((took, file.len()))
}

/// A client with the account's login and the game's achievements asked for, for a run to check
/// them after each frame.
fn start_achievements(system: Option<&str>, rom: &Path) -> Result<crate::emu::cheevos::Client> {
    use crate::emu::cheevos;
    let Some(login) = cheevos::saved_login() else {
        return fail(exit::CONFIG, "not logged in to RetroAchievements; run `romi achievements login`");
    };
    let console = system.and_then(crate::ra::console);
    let Some(console) = console else {
        let what = system.map_or("this game's system".to_string(), |s| format!("the {s} system"));
        return fail(exit::USAGE, format!("RetroAchievements doesn't cover {what}; pass --system for a ROM file"));
    };
    let Some(hash) = crate::ra::hash_file(console, rom) else {
        return fail(exit::UNAVAILABLE, format!("can't work out {}'s RetroAchievements hash", rom.display()));
    };
    cheevos::resend_waiting();
    let client = cheevos::Client::new(console)?;
    client.login_with_token(&login)?;
    client.load_game(&hash)?;
    Ok(client)
}

/// How long a run waits for the game's achievements to load before going on without them.
const LOAD_WAIT: Duration = Duration::from_secs(15);

/// Waits, after the first frame, for the game's achievements to load (or fail to), so the frames
/// after it are checked; gives back how long it took.
fn wait_for_achievements(
    client: &mut crate::emu::cheevos::Client,
    earned: &mut Vec<crate::emu::cheevos::Event>,
) -> Duration {
    use crate::emu::cheevos::Event;
    let started = Instant::now();
    let answered = |events: &[Event]| events.iter().any(|e| matches!(e, Event::Loaded { .. } | Event::Problem(_)));
    while !answered(earned) {
        if started.elapsed() > LOAD_WAIT {
            earned.push(Event::Problem("RetroAchievements didn't answer".into()));
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        client.idle();
        earned.extend(client.take_events());
    }
    started.elapsed()
}

/// What a run's achievements did, as a person reads it.
fn achievements_lines(events: &[crate::emu::cheevos::Event]) -> Vec<String> {
    use crate::emu::cheevos::Event;
    events
        .iter()
        .filter_map(|e| match e {
            Event::Loaded { title, unlocked, total, points, points_total } => Some(format!(
                "RetroAchievements: {title}: {unlocked} of {total} unlocked, {points} of {points_total} points"
            )),
            Event::Unlocked { title, points, .. } => Some(format!("Unlocked: {title} ({points} points)")),
            Event::Mastered { title } => Some(format!("Mastered {title}")),
            Event::Problem(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

/// The same as JSON: the game, what was unlocked in the run, and what went wrong.
fn achievements_json(events: &[crate::emu::cheevos::Event]) -> serde_json::Value {
    use crate::emu::cheevos::Event;
    let mut game = serde_json::Value::Null;
    let (mut unlocked, mut problems) = (Vec::new(), Vec::new());
    let mut mastered = false;
    for e in events {
        match e {
            Event::Loaded { title, unlocked: had, total, points, points_total } => {
                game = serde_json::json!({
                    "title": title, "unlocked": had, "total": total, "points": points, "points_total": points_total,
                });
            }
            Event::Unlocked { title, description, points, .. } => {
                unlocked.push(serde_json::json!({"title": title, "description": description, "points": points}));
            }
            Event::Mastered { .. } => mastered = true,
            Event::Problem(p) => problems.push(p.clone()),
            _ => {}
        }
    }
    serde_json::json!({ "game": game, "unlocked": unlocked, "mastered": mastered, "problems": problems })
}

/// A ROM given by path runs with the command of the first system that lists its extension.
fn rom_file(
    cfg: &Config,
    rom: &Path,
    system: Option<&str>,
) -> Result<(String, PathBuf, Option<crate::launch::Launch>, Option<String>)> {
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
    Ok((name, rom.to_path_buf(), launch, sys.map(|s| s.id.clone())))
}
