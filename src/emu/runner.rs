//! A game running in romi: the core on its own thread, paced by its sound (or by the clock, without
//! sound), with the newest frame kept for whatever draws it. `romi run --live` and the TUI's middle
//! panel both play through this, and draw the frames their own way.

use crate::config::Config;
use crate::emu::audio;
use crate::emu::cheevos::{self, Event as CheevoEvent};
use crate::emu::libretro::{Core, Dirs, Frame, Players};
use crate::emu::remap::{Controls, Remap};
use crate::emu::savestate::{self, Files, State};
use crate::emu::timing;
use crate::emucfg::SharedSaves;
use crate::library::Library;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Loads a core to play `rom` with. It gets RetroArch's system folder and RetroArch's save folder
/// for that core, so what it writes, like FinalBurn Neo's high scores, carries over both ways. The
/// game's own save goes where RetroArch keeps it too, so a game saved in romi carries on in
/// RetroArch and back, and the core runs with the options and remap RetroArch has for it and the
/// game, with `core_options` set in romi over those and `controls` remapped in romi over that
/// (see `remap`). With `shown_height`, how many
/// screen pixels tall the game will be, 3D cores draw at about that size if `sharp_3d` is on.
///
/// `runs_with` is the command the game runs with outside romi. When that's an emulator that keeps
/// saves just as the core does (standalone melonDS or Dolphin, see `emucfg::shared_saves`), the
/// core shares them instead, so the game carries on there.
pub fn load(
    cfg: &Config,
    core: &Path,
    rom: &Path,
    runs_with: Option<&str>,
    controls: &[&Controls],
    core_options: &[&BTreeMap<String, String>],
    shown_height: Option<u32>,
) -> Result<Core> {
    let name = crate::emu::libretro::info(core).map(|info| info.name);
    let shared = name.as_ref().zip(runs_with).and_then(|(name, cmd)| crate::emucfg::shared_saves(cmd, name, rom));
    let shared = shared.map(|shared| match shared {
        SharedSaves::Folder(dir) => Ok(dir),
        SharedSaves::DolphinGc(gc) => dolphin_folder(&crate::config::state_dir().join("dolphin"), &gc),
    });
    let saves = match shared {
        Some(dir) => Some(dir?),
        None => match &cfg.saves_dir {
            Some(dir) => Some(crate::config::expand_path(dir).join(name.as_deref().unwrap_or("core"))),
            None => name.as_ref().and_then(|name| crate::emucfg::retroarch_save_dir(name, rom)),
        },
    };
    if let Some(dir) = &saves {
        std::fs::create_dir_all(dir).with_context(|| format!("can't make {}", dir.display()))?;
    }
    let save_file = save_file(saves.as_deref(), rom);
    let options = options(name.as_deref(), rom, core_options);
    let remap = remap(name.as_deref(), rom, controls);
    let shown_height = shown_height.filter(|_| cfg.sharp_3d);
    Core::load(core, rom, &Dirs { system: cfg.system_dir(), saves, save_file, options, remap, shown_height })
}

/// A folder for Dolphin's core, `dir`, whose memory cards are standalone Dolphin's `gc`: the core
/// keeps them in `User/GC` under it, which is made a link to `gc`. The rest of what the core keeps
/// (its settings, caches, Wii saves) stays in `dir`, out of standalone Dolphin's way.
fn dolphin_folder(dir: &Path, gc: &Path) -> Result<std::path::PathBuf> {
    let user = dir.join("User");
    let link = user.join("GC");
    std::fs::create_dir_all(&user).with_context(|| format!("can't make {}", user.display()))?;
    std::fs::create_dir_all(gc).with_context(|| format!("can't make {}", gc.display()))?;
    match std::fs::read_link(&link) {
        Ok(to) if to == gc => return Ok(dir.to_path_buf()),
        Ok(_) => std::fs::remove_file(&link).with_context(|| format!("can't move {}", link.display()))?,
        // Something that isn't a link is the core's own cards, from before: never thrown away.
        Err(_) if link.symlink_metadata().is_ok() => {
            anyhow::bail!("{} is in the way of Dolphin's memory cards ({})", link.display(), gc.display())
        }
        Err(_) => {}
    }
    std::os::unix::fs::symlink(gc, &link).with_context(|| format!("can't link {}", link.display()))?;
    Ok(dir.to_path_buf())
}

/// The core options a game plays with in `core` (by its name): what RetroArch has for it, with each
/// of `sets` over that in turn.
pub fn options(core: Option<&str>, rom: &Path, sets: &[&BTreeMap<String, String>]) -> BTreeMap<String, String> {
    let mut options = core.map(|name| crate::emucfg::retroarch_core_options(name, rom)).unwrap_or_default();
    for set in sets {
        options.extend(set.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    options
}

/// Dolphin's core's controller for "Wii Remote + Nunchuk": the pad's type (1) and its place in the
/// core's list (3), which it gives for each port with `SET_CONTROLLER_INFO`.
const WII_REMOTE_WITH_NUNCHUK: u32 = 0x301;

/// Whether `rom` is a Wii disc image, in any format Dolphin plays.
fn is_wii_disc(rom: &Path) -> bool {
    nod::read::DiscReader::new(rom, &nod::read::DiscOptions::default()).is_ok_and(|disc| disc.header().is_wii())
}

/// The remap a game plays with in `core` (by its name): RetroArch's for it, with each of `controls`
/// over it in turn.
pub fn remap(core: Option<&str>, rom: &Path, controls: &[&Controls]) -> Remap {
    let retroarch = core.map(|name| crate::emucfg::retroarch_remap(name, rom)).unwrap_or_default();
    let mut remap = controls.iter().fold(retroarch, |remap, controls| remap.with(controls));
    if let Some(device) = core.and_then(|name| crate::emucfg::known_device(name, rom)) {
        remap.devices[0].get_or_insert(device);
    }
    if core == Some("dolphin-emu") && is_wii_disc(rom) {
        // The core plugs in a lone Wii Remote unless told otherwise, which has no stick; a Nunchuk
        // gives the left stick something to be. RetroArch's remap can say what to plug in instead.
        for device in remap.devices.iter_mut().filter(|d| d.is_none()) {
            *device = Some(WII_REMOTE_WITH_NUNCHUK);
        }
    }
    remap
}

/// The buttons remapped in romi for a game in the library, in the order they go over RetroArch's
/// remap: its system's, then its own.
pub fn controls_for<'a>(cfg: &'a Config, lib: &'a Library, g: usize) -> Vec<&'a Controls> {
    cfg.controls.get(&lib.system_of(g).id).into_iter().chain([&lib.games[g].controls]).collect()
}

/// The core options set in romi for a game in the library, in the order they go over RetroArch's:
/// its system's, then its own.
pub fn core_options_for<'a>(cfg: &'a Config, lib: &'a Library, g: usize) -> Vec<&'a BTreeMap<String, String>> {
    cfg.core_options.get(&lib.system_of(g).id).into_iter().chain([&lib.games[g].core_options]).collect()
}

/// Where RetroArch keeps a game's save: the ROM's name with `.srm`, in the save folder, or next to
/// the ROM without one. A zipped game's save is named after the zip.
fn save_file(saves: Option<&Path>, rom: &Path) -> Option<std::path::PathBuf> {
    let mut name = rom.file_stem()?.to_os_string();
    name.push(".srm");
    Some(saves.or(rom.parent())?.join(name))
}

/// What the core thread and whatever draws its frames share.
pub struct Shared {
    /// The newest frame and how many frames have run.
    latest: Mutex<(u64, Frame)>,
    ready: Condvar,
    input: Mutex<Players>,
    quit: AtomicBool,
    /// Held before its first frame, while the launch animation plays.
    paused: AtomicBool,
    /// How loud the sound plays (1 is as the core made it).
    volume: f32,
    muted: AtomicBool,
    /// Milliseconds of sound queued for the device, or `NO_SOUND`.
    queued_ms: AtomicU32,
    /// Why there's no sound, once the core thread has found out.
    sound_error: Mutex<Option<String>>,
    /// Why the game's save couldn't be written, the last time it couldn't.
    save_error: Mutex<Option<String>>,
    /// What's happened with the game's achievements, not yet shown.
    cheevos: Mutex<Vec<CheevoEvent>>,
    /// When each frame was made, if `ROMI_FRAME_LOG` asks (see `timing`).
    made: Option<Mutex<Vec<timing::Made>>>,
    /// What's been asked of save states, for between frames.
    state_ask: Mutex<Option<StateAsk>>,
    /// What's come of it, not yet shown: something to say, or something that went wrong.
    state_news: Mutex<Vec<Result<String, String>>>,
    /// The thread writing the last state saved, which outlives its frame and may be yet to finish.
    state_writer: Mutex<Option<JoinHandle<()>>>,
}

/// Something asked of a game's save states.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StateAsk {
    Save(u8),
    Load(u8),
    /// Goes back to just before the last load, or forward again after an undo.
    Undo,
}

const NO_SOUND: u32 = u32::MAX;

/// How long a game has to stop in, saving a last time and unloading, before it's taken as stuck.
const STOP_WAIT: Duration = Duration::from_secs(3);

/// Whether a game that didn't stop has been left running (see `Shared::stop_and_wait`).
static LEFT_RUNNING: AtomicBool = AtomicBool::new(false);

/// Whether a game that stopped responding is still running, for romi to end without the cleanup
/// that would unload what it's running in.
pub fn left_running() -> bool {
    LEFT_RUNNING.load(Ordering::SeqCst)
}

/// What's said of a game that didn't stop (see `Shared::stop_and_wait`).
pub const STUCK: &str = "the game stopped responding, and is left running until romi quits";

/// A lock that a panic on the other thread can't spoil: nothing it guards is left half-changed.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    /// Starts `core` on its own thread, running until `stop`, with its sound `volume` loud. A
    /// `paused` core warms up (see `WarmUp`), then waits for `resume` before its first frame that's
    /// seen. With `cheevos`, its achievements are checked after every frame.
    pub fn start(
        core: Core,
        volume: f32,
        paused: bool,
        cheevos: Option<cheevos::Setup>,
        states: Option<Files>,
    ) -> Result<(Arc<Shared>, JoinHandle<()>)> {
        let shared = Arc::new(Shared {
            latest: Mutex::new((0, Frame::default())),
            ready: Condvar::new(),
            input: Mutex::new(Players::default()),
            quit: AtomicBool::new(false),
            paused: AtomicBool::new(paused),
            volume,
            muted: AtomicBool::new(false),
            queued_ms: AtomicU32::new(NO_SOUND),
            sound_error: Mutex::new(None),
            save_error: Mutex::new(None),
            cheevos: Mutex::new(Vec::new()),
            made: timing::path().map(|_| Mutex::new(Vec::new())),
            state_ask: Mutex::new(None),
            state_news: Mutex::new(Vec::new()),
            state_writer: Mutex::new(None),
        });
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new().name("core".into()).spawn(move || run_core(core, cheevos, states, &shared))?
        };
        Ok((shared, thread))
    }

    pub fn stop(&self) {
        self.quit.store(true, Ordering::SeqCst);
    }

    /// Stops the core's `thread` and waits for it, for up to `STOP_WAIT`. False if it's still
    /// running then: the core is stuck in a frame, and there's no stopping a thread from outside,
    /// so it's left running, with the core loaded, until romi quits. Play!, after Start in
    /// NeoGeo Battle Coliseum, never came back from its frame, and romi waited for it for good.
    pub fn stop_and_wait(&self, thread: JoinHandle<()>) -> bool {
        self.stop();
        let deadline = Instant::now() + STOP_WAIT;
        while !thread.is_finished() {
            if Instant::now() >= deadline {
                self.finish_writing_state();
                LEFT_RUNNING.store(true, Ordering::SeqCst);
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = thread.join();
        self.finish_writing_state();
        true
    }

    /// Asks for something of the game's save states, in place of anything still waiting.
    pub fn ask_state(&self, ask: StateAsk) {
        *lock(&self.state_ask) = Some(ask);
    }

    /// Passes something on with the news of the game's save states.
    pub fn say_about_states(&self, news: Result<String, String>) {
        lock(&self.state_news).push(news);
    }

    /// What's come of the game's save states since the last time, oldest first.
    pub fn take_state_news(&self) -> Vec<Result<String, String>> {
        std::mem::take(&mut *lock(&self.state_news))
    }

    /// Waits for the state being written, if there is one.
    fn finish_writing_state(&self) {
        let writer = lock(&self.state_writer).take();
        if let Some(writer) = writer {
            let _ = writer.join();
        }
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    /// Holds what each player holds from the next frame on.
    pub fn set_input(&self, players: Players) {
        *lock(&self.input) = players;
    }

    /// The newest frame and its number, once there's one newer than frame number `seen`, waiting up
    /// to `wait` for it.
    pub fn newer_frame(&self, seen: u64, wait: Duration) -> Option<(u64, Frame)> {
        let (mut latest, _) =
            self.ready.wait_timeout_while(lock(&self.latest), wait, |l| l.0 == seen).unwrap_or_else(|e| e.into_inner());
        (latest.0 != seen).then(|| (latest.0, std::mem::take(&mut latest.1)))
    }

    /// Waits up to `wait` for a frame newer than frame number `seen`.
    pub fn wait_newer(&self, seen: u64, wait: Duration) {
        let _ = self.ready.wait_timeout_while(lock(&self.latest), wait, |l| l.0 == seen);
    }

    /// How many frames have run.
    pub fn frames_run(&self) -> u64 {
        lock(&self.latest).0
    }

    pub fn toggle_mute(&self) {
        self.muted.fetch_xor(true, Ordering::Relaxed);
    }

    pub fn muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    /// How much sound is queued ahead of the device, or None without sound.
    pub fn queued_sound(&self) -> Option<Duration> {
        match self.queued_ms.load(Ordering::Relaxed) {
            NO_SOUND => None,
            ms => Some(Duration::from_millis(ms.into())),
        }
    }

    /// Why there's no sound, if there isn't.
    pub fn sound_error(&self) -> Option<String> {
        lock(&self.sound_error).clone()
    }

    /// The same, only once.
    pub fn take_sound_error(&self) -> Option<String> {
        lock(&self.sound_error).take()
    }

    /// Why the game's save couldn't be written, once for each time it couldn't.
    pub fn take_save_error(&self) -> Option<String> {
        lock(&self.save_error).take()
    }

    /// When each frame so far was made, if they're being timed.
    pub fn take_made(&self) -> Vec<timing::Made> {
        self.made.as_ref().map(|m| std::mem::take(&mut *lock(m))).unwrap_or_default()
    }

    /// What's happened with the game's achievements since the last time, oldest first.
    pub fn take_cheevo_events(&self) -> Vec<CheevoEvent> {
        std::mem::take(&mut *lock(&self.cheevos))
    }
}

/// How often a running game's save is written, if it has changed, so a crash loses little.
const SAVE_EVERY: Duration = Duration::from_secs(1);

/// Starts checking a game's achievements: logs in and loads them. The game's memory is found
/// after its first frame (see `Client::frame`).
fn start_cheevos(setup: cheevos::Setup, shared: &Shared) -> Option<cheevos::Client> {
    let problem = |p: String| lock(&shared.cheevos).push(CheevoEvent::Problem(p));
    let client =
        cheevos::Client::new(setup.console).inspect_err(|e| problem(format!("RetroAchievements: {e:#}"))).ok()?;
    cheevos::resend_waiting();
    let started = client.login_with_token(&setup.login).and_then(|()| client.load_game(&setup.hash));
    started.inspect_err(|e| problem(format!("RetroAchievements: {e:#}"))).ok()?;
    Some(client)
}

/// Runs the core until told to quit, playing its sound, at the pace its sound sets (or at its
/// frame rate, without sound).
fn run_core(mut core: Core, cheevos: Option<cheevos::Setup>, states: Option<Files>, shared: &Arc<Shared>) {
    // The stream lives on this thread, so it's opened here.
    let mut sound = audio::Output::open(core.sample_rate, core.fps)
        .inspect_err(|e| *lock(&shared.sound_error) = Some(format!("{e:#}")))
        .ok();
    let mut samples = Vec::new();
    let period = Duration::from_secs_f64(1.0 / core.fps);
    let mut next = Instant::now();
    // How long after its start each frame is handed over (see `hold_for`).
    let mut hold = Duration::ZERO;
    let mut warm = WarmUp::default();
    let mut saved = Instant::now();
    let mut cheevos = cheevos.and_then(|setup| start_cheevos(setup, shared));
    let mut states = states.map(|files| GameStates { files, undo: None });
    let mut frames_run = 0;
    let tell = |client: &cheevos::Client| {
        let events = client.take_events();
        if !events.is_empty() {
            lock(&shared.cheevos).extend(events);
        }
    };
    let save = |core: &mut Core| {
        if let Err(e) = core.save() {
            *lock(&shared.save_error) = Some(format!("{e:#}"));
        }
    };
    while !shared.quit.load(Ordering::Relaxed) {
        if saved.elapsed() >= SAVE_EVERY {
            save(&mut core);
            saved = Instant::now();
        }
        if shared.paused.load(Ordering::Relaxed) {
            if !warm.done() {
                let started = Instant::now();
                core.run_frame(Players::default());
                frames_run += 1;
                warm.ran(started.elapsed(), period);
                continue;
            }
            if let Some(client) = &mut cheevos {
                client.idle();
                tell(client);
            }
            std::thread::sleep(Duration::from_millis(2));
            next = Instant::now();
            continue;
        }
        let ask = lock(&shared.state_ask).take();
        if let (Some(ask), Some(states)) = (ask, &mut states) {
            let news = match frames_run < crate::emu::libretro::FRAMES_BEFORE_STATES {
                true => Err("The game has only just started, so try that again in a moment".to_string()),
                false => states.answer(ask, &mut core, cheevos.as_mut(), shared),
            };
            let loaded = news.is_ok() && !matches!(ask, StateAsk::Save(_));
            // Saving says so once it's written.
            if !matches!((&news, ask), (Ok(_), StateAsk::Save(_))) {
                shared.say_about_states(news);
            }
            if loaded {
                // The time that took isn't made up for with a burst of fast frames.
                next = Instant::now();
                hold = Duration::ZERO;
            }
        }
        let started = Instant::now();
        let input = *lock(&shared.input);
        core.run_frame(input);
        frames_run += 1;
        let ran = started.elapsed();
        if let Some(client) = &mut cheevos {
            client.frame(&core);
            tell(client);
        }
        let wait = match &mut sound {
            Some(sound) => {
                samples.clear();
                core.audio(&mut samples);
                let volume = if shared.muted() { 0.0 } else { shared.volume };
                let lasts = sound.push(&samples, volume);
                shared.queued_ms.store(sound.ahead().as_millis() as u32, Ordering::Relaxed);
                lasts
            }
            None => period,
        };
        let frame = core.frame();
        hold = hold_for(hold, started.elapsed(), period);
        let slot = started + hold;
        if let Some(early) = slot.checked_duration_since(Instant::now()) {
            std::thread::sleep(early);
        }
        {
            let mut latest = lock(&shared.latest);
            latest.0 += 1;
            latest.1 = frame;
        }
        shared.ready.notify_one();
        if let Some(made) = &shared.made {
            let (sound_ahead, sound_dry) = (sound.as_ref().map(|s| s.ahead()), sound.as_ref().map(|s| s.dry()));
            lock(made).push(timing::Made { started, ran, done: Instant::now(), sound_ahead, sound_dry });
        }
        next += wait;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > period * 4 {
            // Too far behind to catch up without a burst of fast frames; carry on from here.
            next = now;
        }
    }
    save(&mut core);
    // The client reads the core's memory, so it goes first.
    drop(cheevos);
}

/// A game's save states, on the thread running its core.
struct GameStates {
    files: Files,
    /// What loading last replaced, for going back to.
    undo: Option<State>,
}

impl GameStates {
    /// Does what was asked, and says what to tell the player.
    fn answer(
        &mut self,
        ask: StateAsk,
        core: &mut Core,
        mut cheevos: Option<&mut cheevos::Client>,
        shared: &Arc<Shared>,
    ) -> Result<String, String> {
        let done = match ask {
            StateAsk::Save(slot) => self.save(slot, core, cheevos, shared).map(|()| String::new()),
            StateAsk::Load(slot) => {
                // Anything still being written to it should be there to read.
                shared.finish_writing_state();
                self.load(slot, core, cheevos.as_deref_mut(), shared)
            }
            StateAsk::Undo => match self.undo.take() {
                Some(state) => self.swap_in(state, core, cheevos, shared).map(|kept| match kept {
                    Some(note) => format!("Went back to before loading{note}"),
                    None => "Went back to before loading".to_string(),
                }),
                None => Ok("There's no load to undo".to_string()),
            },
        };
        done.map_err(|e| format!("{e:#}"))
    }

    /// Takes a state and writes it on another thread, as it can take a second, and says when it's
    /// done. The state is whole in its file or not there, so stopping halfway loses nothing else.
    fn save(
        &self,
        slot: u8,
        core: &mut Core,
        cheevos: Option<&mut cheevos::Client>,
        shared: &Arc<Shared>,
    ) -> Result<()> {
        let mut writer = lock(&shared.state_writer);
        if writer.as_ref().is_some_and(|w| !w.is_finished()) {
            anyhow::bail!("Still saving the last state");
        }
        let state = State { core: core.save_state()?, achievements: cheevos.and_then(|c| c.progress()) };
        let path = self.files.slot(slot);
        let told = shared.clone();
        *writer = Some(std::thread::Builder::new().name("state".into()).spawn(move || {
            let written = savestate::encode(&state).and_then(|file| savestate::pack(&file)).and_then(|file| {
                crate::files::write_atomic(&path, &file).with_context(|| format!("can't write {}", path.display()))
            });
            told.say_about_states(match written {
                Ok(()) => Ok(format!("Saved a state to slot {slot}")),
                Err(e) => Err(format!("{e:#}")),
            });
        })?);
        Ok(())
    }

    fn load(
        &mut self,
        slot: u8,
        core: &mut Core,
        cheevos: Option<&mut cheevos::Client>,
        shared: &Shared,
    ) -> Result<String> {
        let path = self.files.slot(slot);
        let file = match std::fs::read(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(format!("Slot {slot} is empty")),
            Err(e) => return Err(e).with_context(|| format!("can't read {}", path.display())),
        };
        let state = savestate::decode(file).with_context(|| format!("can't load slot {slot}"))?;
        let note = self.swap_in(state, core, cheevos, shared)?;
        Ok(format!("Loaded slot {slot}{}", note.unwrap_or_default()))
    }

    /// Replaces the game as it is with `state`, keeping what it replaced to undo. If that took the
    /// game's in-game save back, a copy of the save is kept first, and it says where.
    fn swap_in(
        &mut self,
        state: State,
        core: &mut Core,
        mut cheevos: Option<&mut cheevos::Client>,
        shared: &Shared,
    ) -> Result<Option<String>> {
        // A core that won't make a state now can still load one; there's only nothing to undo.
        let before = core
            .save_state()
            .ok()
            .map(|core| State { core, achievements: cheevos.as_deref_mut().and_then(|c| c.progress()) });
        // What the game saved in the last second counts as saved.
        if let Err(e) = core.save() {
            *lock(&shared.save_error) = Some(format!("{e:#}"));
        }
        if let Err(e) = core.load_state(&state.core) {
            // A core may have got partway, so the game goes back to where it was.
            if let Some(before) = &before {
                let _ = core.load_state(&before.core);
            }
            return Err(e);
        }
        if let Some(client) = cheevos {
            client.set_progress(state.achievements.as_deref());
        }
        self.undo = before;
        if !core.save_changed() {
            return Ok(None);
        }
        let Some(save) = core.save_path() else { return Ok(None) };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        match self.files.keep_save(save, now) {
            Ok(Some(kept)) => {
                Ok(Some(format!(", which also took back the game's save (copy in {})", crate::config::tilde(&kept))))
            }
            Ok(None) => Ok(None),
            Err(e) => Ok(Some(format!(", which also took back the game's save, and no copy could be made: {e:#}"))),
        }
    }
}

/// A core's first frames, run while it waits behind the launch animation, muted and unseen. Some
/// cores take far longer over their first frames than the rest (Flycast compiles its shaders:
/// 100 ms a frame), which stuttered the game as it came on, and ran its sound dry. Once a few in
/// a row run quickly, or after a second of the game, which starts black, it's ready.
#[derive(Default)]
struct WarmUp {
    frames: u32,
    quick: u32,
}

impl WarmUp {
    /// Quick frames in a row that say the core's warmed up, and the most frames run to get there.
    const QUICK: u32 = 5;
    const MOST: u32 = 60;

    fn ran(&mut self, took: Duration, period: Duration) {
        self.frames += 1;
        self.quick = if took < period / 2 { self.quick + 1 } else { 0 };
    }

    fn done(&self) -> bool {
        self.quick >= WarmUp::QUICK || self.frames >= WarmUp::MOST
    }
}

/// How long after its start to hand a frame over, given the last answer and how long this one
/// took. Some cores take much longer over some frames than others (Flycast: 2 to 12 ms), and
/// handing each over as soon as it was done spaced them as unevenly, which looks choppy. Held
/// as long as the slow ones take, they go out as evenly spaced as they start. It follows a slow
/// frame at once, eases back over a few seconds, and stays under most of a frame, so a frame that
/// runs long is only late, not the start of the core falling behind.
fn hold_for(last: Duration, took: Duration, period: Duration) -> Duration {
    took.max(last.mul_f64(0.995)).min(period * 3 / 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wii_discs_are_known_by_their_header() {
        let dir = std::env::temp_dir().join(format!("romi-wii-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let disc = |name: &str, at: usize, magic: [u8; 4]| {
            let mut image = vec![0u8; 0x60000];
            image[at..at + 4].copy_from_slice(&magic);
            let path = dir.join(name);
            std::fs::write(&path, image).unwrap();
            is_wii_disc(&path)
        };
        assert!(disc("wii.iso", 0x18, [0x5D, 0x1C, 0x9E, 0xA3]));
        assert!(!disc("not.iso", 0x18, [0, 0, 0, 0]), "no magic, no Wii");
        assert!(!is_wii_disc(&dir.join("missing.iso")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cores_warm_up_until_frames_come_quickly_or_a_second_has_gone() {
        let period = Duration::from_millis(16);
        let mut warm = WarmUp::default();
        for took in [100, 100, 3, 3, 100, 3, 3, 3, 3] {
            warm.ran(Duration::from_millis(took), period);
            assert!(!warm.done(), "a slow frame starts the count again");
        }
        warm.ran(Duration::from_millis(3), period);
        assert!(warm.done());
        let mut slow = WarmUp::default();
        for _ in 0..WarmUp::MOST {
            slow.ran(Duration::from_millis(100), period);
        }
        assert!(slow.done());
    }

    #[test]
    fn frames_are_held_as_long_as_slow_ones_take_and_no_more_than_most_of_a_frame() {
        let period = Duration::from_millis(16);
        let ms = Duration::from_millis;
        assert_eq!(hold_for(ms(2), ms(10), period), ms(10), "follows a slow frame at once");
        let eased = hold_for(ms(10), ms(2), period);
        assert!(eased < ms(10) && eased > ms(9), "eases back slowly: {eased:?}");
        assert_eq!(hold_for(ms(2), ms(100), period), ms(12), "never most of a frame");
    }

    #[test]
    fn dolphins_core_shares_standalone_dolphins_memory_cards() {
        let dir = std::env::temp_dir().join(format!("romi-dolphin-folder-test-{}", std::process::id()));
        let (folder, gc, elsewhere) = (dir.join("romi/dolphin"), dir.join("dolphin-emu/GC"), dir.join("old/GC"));
        assert_eq!(dolphin_folder(&folder, &gc).unwrap(), folder);
        assert_eq!(std::fs::read_link(folder.join("User/GC")).unwrap(), gc);
        assert!(gc.is_dir());
        assert_eq!(dolphin_folder(&folder, &gc).unwrap(), folder, "again, as it is");
        assert_eq!(dolphin_folder(&folder, &elsewhere).unwrap(), folder);
        assert_eq!(std::fs::read_link(folder.join("User/GC")).unwrap(), elsewhere, "moved with Dolphin");
        // Cards of the core's own, from before, are never thrown away.
        std::fs::remove_file(folder.join("User/GC")).unwrap();
        std::fs::create_dir(folder.join("User/GC")).unwrap();
        assert!(dolphin_folder(&folder, &gc).unwrap_err().to_string().contains("in the way"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saves_are_named_after_the_rom_in_the_save_folder_or_beside_the_rom() {
        let rom = Path::new("/roms/gb/Super Mario Land 2 (USA, Europe).gb");
        assert_eq!(
            save_file(Some(Path::new("/saves/Gambatte")), rom).unwrap(),
            Path::new("/saves/Gambatte/Super Mario Land 2 (USA, Europe).srm")
        );
        assert_eq!(save_file(None, rom).unwrap(), Path::new("/roms/gb/Super Mario Land 2 (USA, Europe).srm"));
        let zipped = Path::new("/roms/gba/Mr. Bones.zip");
        assert_eq!(save_file(None, zipped).unwrap(), Path::new("/roms/gba/Mr. Bones.srm"));
    }
}
