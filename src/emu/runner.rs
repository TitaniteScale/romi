//! A game running in romi: the core on its own thread, paced by its sound (or by the clock, without
//! sound), with the newest frame kept for whatever draws it. `romi run --live` and the TUI's middle
//! panel both play through this, and draw the frames their own way.

use crate::config::Config;
use crate::emu::audio;
use crate::emu::cheevos::{self, Event as CheevoEvent};
use crate::emu::libretro::{Core, Dirs, Frame};
use crate::emu::timing;
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Loads a core to play `rom` with. It gets RetroArch's system folder and RetroArch's save folder
/// for that core, so what it writes, like FinalBurn Neo's high scores, carries over both ways. The
/// game's own save goes where RetroArch keeps it too, so a game saved in romi carries on in
/// RetroArch and back.
pub fn load(cfg: &Config, core: &Path, rom: &Path) -> Result<Core> {
    let saves = crate::emu::libretro::info(core).and_then(|info| crate::emucfg::retroarch_save_dir(&info.name, rom));
    if let Some(dir) = &saves {
        std::fs::create_dir_all(dir).with_context(|| format!("can't make {}", dir.display()))?;
    }
    let save_file = save_file(saves.as_deref(), rom);
    Core::load(core, rom, &Dirs { system: cfg.system_dir(), saves, save_file })
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
    buttons: AtomicU16,
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
}

const NO_SOUND: u32 = u32::MAX;

/// A lock that a panic on the other thread can't spoil: nothing it guards is left half-changed.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    /// Starts `core` on its own thread, running until `stop`, with its sound `volume` loud. A
    /// `paused` core waits for `resume` before its first frame. With `cheevos`, its achievements
    /// are checked after every frame.
    pub fn start(
        core: Core,
        volume: f32,
        paused: bool,
        cheevos: Option<cheevos::Setup>,
    ) -> Result<(Arc<Shared>, JoinHandle<()>)> {
        let shared = Arc::new(Shared {
            latest: Mutex::new((0, Frame::default())),
            ready: Condvar::new(),
            buttons: AtomicU16::new(0),
            quit: AtomicBool::new(false),
            paused: AtomicBool::new(paused),
            volume,
            muted: AtomicBool::new(false),
            queued_ms: AtomicU32::new(NO_SOUND),
            sound_error: Mutex::new(None),
            save_error: Mutex::new(None),
            cheevos: Mutex::new(Vec::new()),
            made: timing::path().map(|_| Mutex::new(Vec::new())),
        });
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new().name("core".into()).spawn(move || run_core(core, cheevos, &shared))?
        };
        Ok((shared, thread))
    }

    pub fn stop(&self) {
        self.quit.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    /// Holds these buttons (bits from `libretro::button`) from the next frame on.
    pub fn set_buttons(&self, buttons: u16) {
        self.buttons.store(buttons, Ordering::Relaxed);
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
    let started = client.login_with_token(&setup.login).and_then(|()| client.load_game(&setup.hash));
    started.inspect_err(|e| problem(format!("RetroAchievements: {e:#}"))).ok()?;
    Some(client)
}

/// Runs the core until told to quit, playing its sound, at the pace its sound sets (or at its
/// frame rate, without sound).
fn run_core(mut core: Core, cheevos: Option<cheevos::Setup>, shared: &Shared) {
    // The stream lives on this thread, so it's opened here.
    let mut sound = audio::Output::open(core.sample_rate, core.fps)
        .inspect_err(|e| *lock(&shared.sound_error) = Some(format!("{e:#}")))
        .ok();
    let mut samples = Vec::new();
    let period = Duration::from_secs_f64(1.0 / core.fps);
    let mut next = Instant::now();
    let mut saved = Instant::now();
    let mut cheevos = cheevos.and_then(|setup| start_cheevos(setup, shared));
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
            if let Some(client) = &mut cheevos {
                client.idle();
                tell(client);
            }
            std::thread::sleep(Duration::from_millis(2));
            next = Instant::now();
            continue;
        }
        let started = Instant::now();
        core.run_frame(shared.buttons.load(Ordering::Relaxed));
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
        {
            let mut latest = lock(&shared.latest);
            latest.0 += 1;
            latest.1 = frame;
        }
        shared.ready.notify_one();
        if let Some(made) = &shared.made {
            let sound_ahead = sound.as_ref().map(|s| s.ahead());
            lock(made).push(timing::Made { started, ran, done: Instant::now(), sound_ahead });
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

#[cfg(test)]
mod tests {
    use super::*;

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
