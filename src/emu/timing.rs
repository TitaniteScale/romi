//! Frame timings, for finding out why a game plays unevenly. With `ROMI_FRAME_LOG` set to a file,
//! a game played in romi (in the TUI's middle panel, or `romi run --live`) records when the core
//! made each frame, when romi took it, how long it spent scaling and sending it, and when the
//! terminal read it. When the game stops, they're written there as CSV, a row a frame, with a
//! summary beside it in the same name with `.txt`.
//!
//! The terminal deletes each frame's shared memory once it has read it, so a thread watching
//! /dev/shm for deletions knows when that was. What happens after, until the frame is on the
//! screen, is up to the terminal and isn't seen here.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const VAR: &str = "ROMI_FRAME_LOG";

/// Where frame timings go, if they're wanted.
pub fn path() -> Option<PathBuf> {
    std::env::var_os(VAR).filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// A frame the core made.
#[derive(Clone, Copy)]
pub struct Made {
    pub started: Instant,
    /// How long the core took over it.
    pub ran: Duration,
    /// When it was handed over for drawing.
    pub done: Instant,
    pub sound_ahead: Option<Duration>,
    /// The silence played so far for want of sound (see `audio::Output::dry`).
    pub sound_dry: Option<Duration>,
}

/// A frame romi sent the terminal, and when each step of sending it was done.
pub struct Sent {
    /// Its number, counting from 1, as the core made them.
    pub frame: u64,
    pub taken: Instant,
    /// Scaled, filtered and its corners rounded.
    pub prepared: Instant,
    /// In shared memory (or encoded, when the pixels go in the escape sequence).
    pub copied: Instant,
    /// Written to the terminal.
    pub written: Instant,
    pub size: (u32, u32),
    pub bytes: usize,
}

/// What's recorded while a game plays.
pub struct Recorder {
    path: PathBuf,
    /// What's playing, and how it's drawn, for the summary.
    about: String,
    sent: Vec<Sent>,
    /// Shared memory names, and the frame each holds.
    names: HashMap<String, u64>,
    /// Ones romi deleted itself, because the terminal never read them.
    unread: Vec<String>,
    watcher: Option<Watcher>,
}

impl Recorder {
    /// Starts recording, if `ROMI_FRAME_LOG` asks for it. `about` says what's playing.
    pub fn start(about: String) -> Option<Recorder> {
        let path = path()?;
        Some(Recorder {
            path,
            about,
            sent: Vec::new(),
            names: HashMap::new(),
            unread: Vec::new(),
            watcher: Watcher::start().ok(),
        })
    }

    pub fn sent(&mut self, sent: Sent) {
        self.sent.push(sent);
    }

    /// Notes that `frame` went in the shared memory object `name`.
    pub fn handed_over(&mut self, frame: u64, name: &str) {
        self.names.insert(name.trim_start_matches('/').to_string(), frame);
    }

    /// Notes that romi deleted `name` because the terminal hadn't read it.
    pub fn never_read(&mut self, name: &str) {
        self.unread.push(name.trim_start_matches('/').to_string());
    }

    /// Writes the timings and their summary, given the frames the core made, and says where.
    pub fn finish(mut self, made: &[Made]) -> Result<PathBuf> {
        let deleted = self.watcher.take().map(Watcher::stop).unwrap_or_default();
        for name in &self.unread {
            self.names.remove(name);
        }
        let mut read: HashMap<u64, Instant> = HashMap::new();
        for (name, at) in deleted {
            if let Some(&frame) = self.names.get(&name) {
                read.entry(frame).or_insert(at);
            }
        }
        let epoch = made.first().map(|m| m.started).or(self.sent.first().map(|s| s.taken)).unwrap_or_else(Instant::now);
        let ms = |t: Instant| t.saturating_duration_since(epoch).as_secs_f64() * 1000.0;
        let mut csv = String::from(
            "frame,made_ms,run_ms,sound_ahead_ms,sound_dry_ms,taken_ms,prepared_ms,copied_ms,written_ms,read_ms,width,height,bytes\n",
        );
        let sent: HashMap<u64, &Sent> = self.sent.iter().map(|s| (s.frame, s)).collect();
        for (i, m) in made.iter().enumerate() {
            let frame = i as u64 + 1;
            let ahead = m.sound_ahead.map(|d| format!("{}", d.as_millis())).unwrap_or_default();
            let dry = m.sound_dry.map(|d| format!("{}", d.as_millis())).unwrap_or_default();
            let _ = write!(csv, "{frame},{:.3},{:.3},{ahead},{dry}", ms(m.done), m.ran.as_secs_f64() * 1000.0);
            match sent.get(&frame) {
                Some(s) => {
                    let read = read.get(&frame).map(|&t| format!("{:.3}", ms(t))).unwrap_or_default();
                    let _ = writeln!(
                        csv,
                        ",{:.3},{:.3},{:.3},{:.3},{read},{},{},{}",
                        ms(s.taken),
                        ms(s.prepared),
                        ms(s.copied),
                        ms(s.written),
                        s.size.0,
                        s.size.1,
                        s.bytes,
                    );
                }
                None => csv.push_str(",,,,,,,,\n"),
            }
        }
        let summary = summarize(&self.about, made, &self.sent, &read, self.unread.len());
        std::fs::write(&self.path, csv).with_context(|| format!("can't write {}", self.path.display()))?;
        let txt = summary_path(&self.path);
        std::fs::write(&txt, summary).with_context(|| format!("can't write {}", txt.display()))?;
        Ok(self.path)
    }
}

/// Where the summary of timings written to `csv` goes.
pub fn summary_path(csv: &Path) -> PathBuf {
    csv.with_extension("txt")
}

/// Watches /dev/shm for this process's frames being deleted, and notes when.
struct Watcher {
    deleted: Arc<Mutex<Vec<(String, Instant)>>>,
    thread: JoinHandle<()>,
    stop_name: String,
}

impl Watcher {
    fn start() -> Result<Watcher> {
        use rustix::fs::inotify;
        let fd = inotify::init(inotify::CreateFlags::CLOEXEC)?;
        inotify::add_watch(&fd, "/dev/shm", inotify::WatchFlags::DELETE)?;
        let prefix = format!("romi-{}-", std::process::id());
        let stop_name = format!("{prefix}timing-stop");
        let deleted = Arc::new(Mutex::new(Vec::new()));
        let thread = {
            let (deleted, stop_name) = (deleted.clone(), stop_name.clone());
            std::thread::Builder::new().name("frame-timing".into()).spawn(move || {
                let mut buf = [std::mem::MaybeUninit::uninit(); 4096];
                let mut events = inotify::Reader::new(&fd, &mut buf);
                while let Ok(event) = events.next() {
                    let at = Instant::now();
                    let Some(name) = event.file_name().and_then(|n| n.to_str().ok()) else { continue };
                    if name == stop_name {
                        return;
                    }
                    if name.starts_with(&prefix) {
                        deleted.lock().unwrap_or_else(|e| e.into_inner()).push((name.to_string(), at));
                    }
                }
            })?
        };
        Ok(Watcher { deleted, thread, stop_name })
    }

    /// Stops watching, and gives back what was deleted, and when.
    fn stop(self) -> Vec<(String, Instant)> {
        // Deleting one more wakes the thread up to see it's done.
        let name = format!("/{}", self.stop_name);
        if crate::emu::kitty::write_shm(&name, &[0]).is_ok() {
            crate::emu::kitty::remove_shm(&name);
            let _ = self.thread.join();
        }
        std::mem::take(&mut *self.deleted.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Numbers in milliseconds, for summing up.
struct Spread(Vec<f64>);

impl Spread {
    fn new(mut v: Vec<f64>) -> Spread {
        v.sort_by(f64::total_cmp);
        Spread(v)
    }

    fn mean(&self) -> f64 {
        self.0.iter().sum::<f64>() / self.0.len().max(1) as f64
    }

    fn sd(&self) -> f64 {
        let mean = self.mean();
        (self.0.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / self.0.len().max(1) as f64).sqrt()
    }

    fn pct(&self, p: f64) -> f64 {
        match self.0.len() {
            0 => 0.0,
            n => self.0[((n - 1) as f64 * p).round() as usize],
        }
    }

    fn max(&self) -> f64 {
        self.0.last().copied().unwrap_or(0.0)
    }

    /// Mean, 99th percentile and worst.
    fn line(&self) -> String {
        format!("mean {:.2} ms, p99 {:.2}, worst {:.2}", self.mean(), self.pct(0.99), self.max())
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// The gaps between successive times, in milliseconds.
fn gaps(times: impl IntoIterator<Item = Instant>) -> Vec<f64> {
    let times: Vec<Instant> = times.into_iter().collect();
    times.windows(2).map(|w| ms(w[1].saturating_duration_since(w[0]))).collect()
}

/// How evenly spaced `gaps` are, for frames meant to come every `period` ms: their spread, and
/// how many came over half a frame late (so the frame before stayed up for two of the screen's
/// refreshes, or the one after was never seen).
fn evenness(gaps: &[f64], period: f64) -> String {
    let spread = Spread::new(gaps.to_vec());
    let late = gaps.iter().filter(|&&g| g > period * 1.5).count();
    format!(
        "mean {:.2} ms, sd {:.2}, p1 {:.2}, p99 {:.2}, worst {:.2}; {late} over {:.0} ms ({:.1}%)",
        spread.mean(),
        spread.sd(),
        spread.pct(0.01),
        spread.pct(0.99),
        spread.max(),
        period * 1.5,
        late as f64 * 100.0 / gaps.len().max(1) as f64,
    )
}

/// Sums up the frames made from the first one sent on: the ones before were the launch
/// animation's, or made before there was anywhere to draw them.
fn summarize(about: &str, all_made: &[Made], sent: &[Sent], read: &HashMap<u64, Instant>, unread: usize) -> String {
    let mut out = format!("romi frame timings: {about}\n\n");
    let before = sent.first().map_or(0, |s| s.frame as usize - 1).min(all_made.len());
    let made = &all_made[before..];
    if before > 0 {
        let _ = writeln!(out, "(leaving out the {before} frames made before the first was sent)");
    }
    let Some((first, last)) = made.first().zip(made.last()) else {
        out.push_str("The core made no frames.\n");
        return out;
    };
    let secs = last.done.saturating_duration_since(first.started).as_secs_f64().max(1e-9);
    let made_gaps = gaps(made.iter().map(|m| m.done));
    let period = Spread::new(made_gaps.clone()).mean().max(1e-3);
    let _ = writeln!(out, "played    {secs:.1} s, {} frames at {:.2} fps", made.len(), made.len() as f64 / secs);
    let _ = writeln!(out, "core      a frame runs in {}", Spread::new(made.iter().map(|m| ms(m.ran)).collect()).line());
    let _ = writeln!(out, "          made every {}", evenness(&made_gaps, period));
    let ahead: Vec<f64> = made.iter().filter_map(|m| m.sound_ahead.map(ms)).collect();
    if !ahead.is_empty() {
        let ahead = Spread::new(ahead);
        let _ = writeln!(
            out,
            "          sound queued {:.0} ms ahead on average, {:.0} at least",
            ahead.mean(),
            ahead.pct(0.0)
        );
    }
    // Each frame that found more silence played than the last is a gap.
    let dry: Vec<Duration> = made.iter().filter_map(|m| m.sound_dry).collect();
    if let Some((first, last)) = dry.first().zip(dry.last()) {
        let gaps = dry.windows(2).filter(|w| w[1] > w[0]).count();
        let _ = writeln!(out, "          sound ran dry {gaps} times, {:.0} ms in all", ms(last.saturating_sub(*first)));
    }
    out.push('\n');
    if sent.is_empty() {
        out.push_str("No frames were sent.\n");
        return out;
    }
    let size = sent[sent.len() / 2].size;
    let bytes = sent.iter().map(|s| s.bytes as f64).sum::<f64>() / sent.len() as f64;
    let skipped = made.len().saturating_sub(sent.len());
    let _ = writeln!(
        out,
        "sent      {} frames, {} never ({:.1}%), at {}×{}, {:.1} MB a frame, {:.0} MB/s",
        sent.len(),
        skipped,
        skipped as f64 * 100.0 / made.len() as f64,
        size.0,
        size.1,
        bytes / 1e6,
        bytes * sent.len() as f64 / secs / 1e6,
    );
    let _ = writeln!(out, "          sent every {}", evenness(&gaps(sent.iter().map(|s| s.written)), period));
    let made_at = |s: &Sent| all_made.get(s.frame as usize - 1).map_or(s.taken, |m| m.done);
    let stage = |f: &dyn Fn(&Sent) -> Duration| Spread::new(sent.iter().map(|s| ms(f(s))).collect()).line();
    let _ = writeln!(
        out,
        "steps     made → taken by romi:     {}",
        stage(&|s| s.taken.saturating_duration_since(made_at(s)))
    );
    let _ = writeln!(
        out,
        "          scaled and filtered in:   {}",
        stage(&|s| s.prepared.saturating_duration_since(s.taken))
    );
    let _ = writeln!(
        out,
        "          copied for the terminal:  {}",
        stage(&|s| s.copied.saturating_duration_since(s.prepared))
    );
    let _ = writeln!(
        out,
        "          written to the terminal:  {}",
        stage(&|s| s.written.saturating_duration_since(s.copied))
    );
    let _ = writeln!(
        out,
        "          made → written:           {}",
        stage(&|s| s.written.saturating_duration_since(made_at(s)))
    );
    out.push('\n');
    let read_sent: Vec<(&Sent, Instant)> = sent.iter().filter_map(|s| read.get(&s.frame).map(|&t| (s, t))).collect();
    if read_sent.is_empty() {
        let why = match unread {
            0 => {
                "the terminal never deleted a frame's shared memory (or the frames didn't go through it), so when it read them isn't known"
            }
            _ => "the terminal read none of the frames",
        };
        let _ = writeln!(out, "terminal  {why}.");
        return out;
    }
    let after = Spread::new(read_sent.iter().map(|(s, t)| ms(t.saturating_duration_since(s.written))).collect());
    let _ = writeln!(out, "terminal  read {} frames, {unread} never", read_sent.len());
    let _ = writeln!(out, "          read after it was sent: {}", after.line());
    let _ = writeln!(out, "          read every {}", evenness(&gaps(read_sent.iter().map(|&(_, t)| t)), period));
    let made_to_read =
        Spread::new(read_sent.iter().map(|&(s, t)| ms(t.saturating_duration_since(made_at(s)))).collect());
    let _ = writeln!(out, "          made → read: {}", made_to_read.line());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evenness_counts_frames_more_than_half_a_frame_late() {
        let line = evenness(&[16.0, 16.0, 30.0, 4.0, 16.0], 16.0);
        assert!(line.contains("1 over 24 ms (20.0%)"), "{line}");
    }

    #[test]
    fn timings_are_written_with_a_summary() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let made: Vec<Made> = (0..4)
            .map(|i| Made {
                started: at(i * 16),
                ran: Duration::from_millis(2),
                done: at(i * 16 + 2),
                sound_ahead: None,
                sound_dry: None,
            })
            .collect();
        let path = std::env::temp_dir().join(format!("romi-timing-test-{}.csv", std::process::id()));
        let mut rec = Recorder {
            path: path.clone(),
            about: "test".into(),
            sent: Vec::new(),
            names: HashMap::new(),
            unread: Vec::new(),
            watcher: None,
        };
        for frame in [1, 2, 4] {
            let t = at(frame * 16 - 13);
            rec.sent(Sent {
                frame,
                taken: t,
                prepared: t + Duration::from_millis(5),
                copied: t + Duration::from_millis(7),
                written: t + Duration::from_millis(8),
                size: (480, 320),
                bytes: 480 * 320 * 3,
            });
        }
        rec.finish(&made).unwrap();
        let csv = std::fs::read_to_string(&path).unwrap();
        let summary = std::fs::read_to_string(summary_path(&path)).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(summary_path(&path)).unwrap();
        let rows: Vec<&str> = csv.lines().collect();
        assert_eq!(rows.len(), 5);
        assert!(rows[1].starts_with("1,2.000,2.000,,,3.000,8.000,10.000,11.000,,480,320,460800"), "{}", rows[1]);
        assert_eq!(rows[3], "3,34.000,2.000,,,,,,,,,,");
        assert!(summary.contains("3 frames, 1 never (25.0%)"), "{summary}");
        assert!(summary.contains("scaled and filtered in:   mean 5.00 ms"), "{summary}");
    }
}
