//! Sound for games played inside romi: out of the speakers while playing, or into a WAV file for
//! `romi run --wav`.
//!
//! While playing, the core thread resamples each frame's sound to the device's rate and queues
//! it, and the device's own thread plays from the queue. The sound also sets the game's pace:
//! each frame takes as long as the sound it made, so the game runs at the speed the sound card
//! plays it. (A core's sample rate is reliable where its frame rate isn't always: mGBA says 61.2
//! fps for Game Boy games, which run at 59.7.) The clock and the sound card's clock still drift
//! apart a little, so frames are stretched or shortened by up to half a percent to keep the
//! queue near `TARGET`. Frames stay evenly spaced, as the picture needs, and the pitch is exact.

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

/// Sound kept queued ahead of the device. Less risks gaps when a frame runs late; more puts the
/// sound further behind the picture.
const TARGET: Duration = Duration::from_millis(40);
/// What the device is asked to buffer on its side, per callback.
const PERIOD: Duration = Duration::from_millis(10);
/// The most a frame is stretched or shortened by to keep the queue at `TARGET`.
const MAX_NUDGE: f64 = 0.005;

/// A stream to the default output device.
pub struct Output {
    _stream: cpal::Stream,
    /// Interleaved left and right samples at the device's rate.
    queue: rtrb::Producer<f32>,
    capacity: usize,
    rate: u32,
    core_rate: f64,
    resampler: Resampler,
    scratch: Vec<f32>,
    /// How long a frame's sound lasts, averaged over recent frames, in seconds.
    frame_time: f64,
    /// How much was queued before the last frame's sound.
    ahead: Duration,
}

impl Output {
    /// Opens the default output device for sound at `core_rate` samples per second, from a core
    /// that says it runs at `fps`.
    pub fn open(core_rate: f64, fps: f64) -> Result<Output> {
        if !core_rate.is_finite() || core_rate <= 0.0 {
            bail!("the core gave no sample rate");
        }
        let device = cpal::default_host().default_output_device().context("no sound output device")?;
        let supported = device.default_output_config().context("can't ask the sound device how to play")?;
        let rate = supported.sample_rate();
        let channels = supported.channels();
        let capacity = rate as usize; // half a second of stereo
        let (queue, from_queue) = rtrb::RingBuffer::new(capacity);
        let fixed = cpal::BufferSize::Fixed((u64::from(rate) * PERIOD.as_millis() as u64 / 1000) as u32);
        let mut config = cpal::StreamConfig { channels, sample_rate: rate, buffer_size: fixed };
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, &mut config, from_queue),
            cpal::SampleFormat::I16 => build::<i16>(&device, &mut config, from_queue),
            cpal::SampleFormat::I32 => build::<i32>(&device, &mut config, from_queue),
            cpal::SampleFormat::U8 => build::<u8>(&device, &mut config, from_queue),
            other => bail!("the sound device wants {other} samples, which romi can't make"),
        }?;
        stream.play().context("can't start the sound")?;
        let mut output = Output {
            _stream: stream,
            queue,
            capacity,
            rate,
            core_rate,
            resampler: Resampler::default(),
            scratch: Vec::new(),
            frame_time: 1.0 / fps,
            ahead: Duration::ZERO,
        };
        output.pad();
        Ok(output)
    }

    /// Sound queued and not yet taken by the device.
    fn queued(&self) -> Duration {
        let samples = self.capacity - self.queue.slots();
        Duration::from_secs_f64(samples as f64 / 2.0 / f64::from(self.rate))
    }

    /// How much sound was queued before the last frame's.
    pub fn ahead(&self) -> Duration {
        self.ahead
    }

    /// Queues a frame's sound (interleaved left and right samples at the core's rate), `volume`
    /// loud (1 is as the core made it). Returns how long the game should take over this frame:
    /// as long as its sound lasts, a little longer while the queue is ahead of `TARGET` and a
    /// little shorter while it's behind.
    pub fn push(&mut self, samples: &[i16], volume: f32) -> Duration {
        // Run dry, there was a gap anyway; start again a whole `TARGET` ahead.
        if self.queue.slots() == self.capacity {
            self.pad();
        }
        self.ahead = self.queued();
        // Cores make about the same amount of sound each frame, but not exactly. One that makes
        // none keeps to the frame rate it gave.
        let lasts = samples.len() as f64 / 2.0 / self.core_rate;
        if lasts > 0.0 {
            self.frame_time += (lasts - self.frame_time) * 0.05;
        }
        // Far ahead, the nudge would take many seconds to catch up, so this frame's sound is
        // skipped instead. It happens when the device is slow to wake up, at the start, which is
        // mostly silent anyway.
        if self.ahead <= TARGET * 3 / 2 {
            self.scratch.clear();
            self.resampler.run(samples, f64::from(self.rate) / self.core_rate, volume, &mut self.scratch);
            // When the device has stopped taking sound, what doesn't fit is dropped.
            let n = self.queue.slots().min(self.scratch.len()) & !1;
            let _ = self.queue.push_entire_slice(&self.scratch[..n]);
        }
        let ahead = (self.ahead.as_secs_f64() - TARGET.as_secs_f64()) / TARGET.as_secs_f64();
        Duration::from_secs_f64(self.frame_time * (1.0 + MAX_NUDGE * ahead.clamp(-1.0, 1.0)))
    }

    /// Fills the queue with silence up to `TARGET`.
    fn pad(&mut self) {
        let frames = (TARGET.as_secs_f64() * f64::from(self.rate)) as usize;
        let n = (frames * 2).min(self.queue.slots());
        let _ = self.queue.push_entire_slice(&vec![0.0; n]);
    }
}

/// A stream that plays from `queue`, putting left and right in the device's first two channels
/// (or mixing them, for a mono device). Asks for `config`'s buffer size, or the device's own if
/// it won't take that.
fn build<T>(device: &cpal::Device, config: &mut cpal::StreamConfig, queue: rtrb::Consumer<f32>) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let queue = std::sync::Arc::new(std::sync::Mutex::new(queue));
    let open = |config: cpal::StreamConfig| {
        let queue = queue.clone();
        let channels = usize::from(config.channels);
        device.build_output_stream(
            config,
            move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
                // Only this callback ever locks it, so it never waits.
                let mut queue = queue.lock().unwrap_or_else(|e| e.into_inner());
                for frame in out.chunks_mut(channels) {
                    let (l, r) = match queue.slots() >= 2 {
                        true => (queue.pop().unwrap_or(0.0), queue.pop().unwrap_or(0.0)),
                        false => (0.0, 0.0),
                    };
                    match frame {
                        [mono] => *mono = T::from_sample((l + r) / 2.0),
                        [left, right, rest @ ..] => {
                            *left = T::from_sample(l);
                            *right = T::from_sample(r);
                            rest.fill(T::from_sample(0.0));
                        }
                        [] => {}
                    }
                }
            },
            // Errors can't be shown while the game owns the terminal; a device that stops just
            // stops taking sound, and the game plays on silently.
            |_| {},
            None,
        )
    };
    match open(*config) {
        Ok(stream) => Ok(stream),
        Err(_) => {
            config.buffer_size = cpal::BufferSize::Default;
            open(*config).context("can't open the sound device")
        }
    }
}

/// Changes the sample rate of interleaved stereo sound, by straight lines between samples: plenty
/// for the sound of 2D systems. Keeps its place between calls, so frames join up seamlessly.
#[derive(Default)]
struct Resampler {
    /// The last sample taken in.
    last: [f32; 2],
    /// How far past `last` the next sample out falls, in samples in.
    pos: f64,
}

impl Resampler {
    /// Appends `input` resampled by `ratio` (samples out per sample in) and scaled by `volume`.
    fn run(&mut self, input: &[i16], ratio: f64, volume: f32, out: &mut Vec<f32>) {
        let step = 1.0 / ratio;
        let gain = volume / 32768.0;
        for pair in input.as_chunks::<2>().0 {
            let next = [f32::from(pair[0]) * gain, f32::from(pair[1]) * gain];
            while self.pos < 1.0 {
                let t = self.pos as f32;
                out.push(self.last[0] + (next[0] - self.last[0]) * t);
                out.push(self.last[1] + (next[1] - self.last[1]) * t);
                self.pos += step;
            }
            self.pos -= 1.0;
            self.last = next;
        }
    }
}

// ---- WAV files ----

/// 16-bit stereo sound written to a WAV file as it comes. The header's sizes are filled in by
/// `finish`.
pub struct Wav {
    file: BufWriter<File>,
    rate: u32,
    bytes: u32,
}

impl Wav {
    pub fn create(path: &Path, rate: f64) -> Result<Wav> {
        let rate = rate.round() as u32;
        let mut file = BufWriter::new(File::create(path).with_context(|| format!("can't write {}", path.display()))?);
        file.write_all(&wav_header(rate, 0))?;
        Ok(Wav { file, rate, bytes: 0 })
    }

    /// Adds interleaved left and right samples.
    pub fn write(&mut self, samples: &[i16]) -> Result<()> {
        for s in samples {
            self.file.write_all(&s.to_le_bytes())?;
        }
        self.bytes = self.bytes.saturating_add((samples.len() * 2) as u32);
        Ok(())
    }

    /// Fills in the header and closes the file. How long the sound is.
    pub fn finish(mut self) -> Result<Duration> {
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&wav_header(self.rate, self.bytes))?;
        self.file.flush()?;
        Ok(Duration::from_secs_f64(f64::from(self.bytes) / 4.0 / f64::from(self.rate.max(1))))
    }
}

/// The 44-byte header of a 16-bit stereo PCM WAV file with `bytes` bytes of samples.
fn wav_header(rate: u32, bytes: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    let mut at = 0;
    let mut put = |b: &[u8]| {
        h[at..at + b.len()].copy_from_slice(b);
        at += b.len();
    };
    put(b"RIFF");
    put(&bytes.saturating_add(36).to_le_bytes());
    put(b"WAVEfmt ");
    put(&16u32.to_le_bytes()); // size of the format chunk
    put(&1u16.to_le_bytes()); // PCM
    put(&2u16.to_le_bytes()); // channels
    put(&rate.to_le_bytes());
    put(&(rate * 4).to_le_bytes()); // bytes per second
    put(&4u16.to_le_bytes()); // bytes per frame
    put(&16u16.to_le_bytes()); // bits per sample
    put(b"data");
    put(&bytes.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_keeps_the_rate_and_joins_frames_up() {
        let mut r = Resampler::default();
        let mut out = Vec::new();
        let frame = [16384i16, -16384].repeat(32768 / 60);
        for _ in 0..60 {
            r.run(&frame, 48000.0 / 32768.0, 1.0, &mut out);
        }
        // About 48000 samples out per 32768 in, and past the ramp up from silence, a steady
        // signal stays steady.
        let pairs = out.len() / 2;
        assert!((pairs as i64 - 48000 * (32768 / 60 * 60) / 32768).abs() <= 1, "{pairs}");
        assert!(out[4..].as_chunks::<2>().0.iter().all(|&[l, r]| l == 0.5 && r == -0.5));
    }

    #[test]
    fn resampling_draws_straight_lines_between_samples() {
        let mut r = Resampler::default();
        let mut out = Vec::new();
        r.run(&[0, 0, 32767, 32767], 2.0, 1.0, &mut out);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(left.len(), 4);
        assert!((left[3] - 32767.0 / 65536.0).abs() < 1e-6, "{left:?}");
    }

    #[test]
    fn wav_files_get_their_sizes_when_finished() {
        let path = std::env::temp_dir().join(format!("romi-wav-test-{}.wav", std::process::id()));
        let mut wav = Wav::create(&path, 32768.4).unwrap();
        wav.write(&[1, -1, 2, -2]).unwrap();
        wav.write(&[3, -3]).unwrap();
        let len = wav.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(bytes[..44], wav_header(32768, 12));
        assert_eq!(bytes[44..], [1, 0, 255, 255, 2, 0, 254, 255, 3, 0, 253, 255]);
        assert_eq!(len, Duration::from_secs_f64(3.0 / 32768.0));
    }

    #[test]
    fn wav_headers_describe_16_bit_stereo() {
        let h = wav_header(32768, 1000);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 1036);
        assert_eq!(&h[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([h[22], h[23]]), 2);
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 32768);
        assert_eq!(u32::from_le_bytes(h[28..32].try_into().unwrap()), 131072);
        assert_eq!(&h[36..40], b"data");
        assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 1000);
    }
}
