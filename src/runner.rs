//! A game running in romi: the core on its own thread, paced by its sound (or by the clock, without
//! sound), with the newest frame kept for whatever draws it. `romi run --live` and the TUI's middle
//! panel both play through this, and draw the frames their own way.

use crate::audio;
use crate::config::Config;
use crate::retro::{Core, Dirs, Frame};
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Loads a core to play `rom` with. It gets RetroArch's system folder and RetroArch's save folder
/// for that core, so what it writes, like FinalBurn Neo's high scores, carries over both ways.
pub fn load(cfg: &Config, core: &Path, rom: &Path) -> Result<Core> {
    let saves = crate::retro::info(core).and_then(|info| crate::emucfg::retroarch_save_dir(&info.name, rom));
    if let Some(dir) = &saves {
        std::fs::create_dir_all(dir).with_context(|| format!("can't make {}", dir.display()))?;
    }
    Core::load(core, rom, &Dirs { system: cfg.system_dir(), saves })
}

/// What the core thread and whatever draws its frames share.
pub struct Shared {
    /// The newest frame and how many frames have run.
    latest: Mutex<(u64, Frame)>,
    ready: Condvar,
    buttons: AtomicU16,
    quit: AtomicBool,
    /// How loud the sound plays (1 is as the core made it).
    volume: f32,
    muted: AtomicBool,
    /// Milliseconds of sound queued for the device, or `NO_SOUND`.
    queued_ms: AtomicU32,
    /// Why there's no sound, once the core thread has found out.
    sound_error: Mutex<Option<String>>,
}

const NO_SOUND: u32 = u32::MAX;

/// A lock that a panic on the other thread can't spoil: nothing it guards is left half-changed.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    /// Starts `core` on its own thread, running until `stop`, with its sound `volume` loud.
    pub fn start(core: Core, volume: f32) -> Result<(Arc<Shared>, JoinHandle<()>)> {
        let shared = Arc::new(Shared {
            latest: Mutex::new((0, Frame::default())),
            ready: Condvar::new(),
            buttons: AtomicU16::new(0),
            quit: AtomicBool::new(false),
            volume,
            muted: AtomicBool::new(false),
            queued_ms: AtomicU32::new(NO_SOUND),
            sound_error: Mutex::new(None),
        });
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new().name("core".into()).spawn(move || run_core(core, &shared))?
        };
        Ok((shared, thread))
    }

    pub fn stop(&self) {
        self.quit.store(true, Ordering::SeqCst);
    }

    /// Holds these buttons (bits from `retro::button`) from the next frame on.
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
}

/// Runs the core until told to quit, playing its sound, at the pace its sound sets (or at its
/// frame rate, without sound).
fn run_core(mut core: Core, shared: &Shared) {
    // The stream lives on this thread, so it's opened here.
    let mut sound = audio::Output::open(core.sample_rate, core.fps)
        .inspect_err(|e| *lock(&shared.sound_error) = Some(format!("{e:#}")))
        .ok();
    let mut samples = Vec::new();
    let period = Duration::from_secs_f64(1.0 / core.fps);
    let mut next = Instant::now();
    while !shared.quit.load(Ordering::Relaxed) {
        core.run_frame(shared.buttons.load(Ordering::Relaxed));
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
        next += wait;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > period * 4 {
            // Too far behind to catch up without a burst of fast frames; carry on from here.
            next = now;
        }
    }
}
