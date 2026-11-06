//! Games with no libretro core, played in the middle panel anyway: the game's own command runs in
//! gamescope with no screen (its headless backend), gamescope streams what it draws over PipeWire,
//! and each frame of that stream becomes the newest frame in `Shared`, for the panel to show as it
//! shows a core's.
//!
//! The game reads the controllers itself, and plays its own sound. romi only has its picture, so
//! there are no save states or achievements here.
//!
//! An experiment, for now: see ROADMAP.md, "Play anything in the panel".

use crate::emu::libretro::Frame;
use crate::emu::runner::Shared;
use crate::launch::Prepared;
use anyhow::{Context, Result, bail};
use pipewire as pw;
use pw::spa;
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The size gamescope gives the game and streams it at. The Xbox 360's own: most of its games
/// draw at 1280×720.
pub const SIZE: (u32, u32) = (1280, 720);
/// The rate it's asked to run the game at.
const RATE: u32 = 60;
/// How long gamescope gets to start streaming.
const START_WAIT: Duration = Duration::from_secs(20);
/// How often the stream's thread checks whether the game's been stopped or has exited.
const CHECK_EVERY: Duration = Duration::from_millis(50);
/// How long the game gets to close when it's stopped, before it's killed.
const CLOSE_WAIT: Duration = Duration::from_secs(3);
/// What gamescope says when its stream is ready, before the PipeWire node's id.
const READY: &str = "stream available on node ID: ";

/// Whether gamescope is installed, which playing this way needs.
pub fn available() -> bool {
    crate::launch::find_program("gamescope").is_some()
}

/// Starts `game` in a hidden gamescope, with its output in `log`, and a thread that puts each frame
/// it draws in `shared`. When the game exits by itself, `shared` says so (`Shared::take_ended`).
pub fn start(game: &Prepared, log: &Path, shared: Arc<Shared>) -> Result<JoinHandle<()>> {
    if !available() {
        bail!("gamescope isn't installed, and playing this in romi needs it");
    }
    let (w, h) = SIZE;
    let size = |n: u32| n.to_string();
    let mut argv: Vec<String> = ["gamescope", "--backend", "headless", "-w"].map(String::from).into();
    argv.extend([size(w), "-h".into(), size(h), "-W".into(), size(w), "-H".into(), size(h)]);
    argv.extend(["-r".into(), RATE.to_string(), "--".into()]);
    argv.extend(game.argv.iter().cloned());
    let wrapped = Prepared { argv, cwd: game.cwd.clone(), env: game.env.clone() };

    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let out = std::fs::File::create(log)?;
    let mut cmd = wrapped.command();
    // Its own process group, to stop gamescope, the game and its Xwayland together.
    cmd.stdin(Stdio::null()).stdout(out.try_clone()?).stderr(Stdio::piped()).process_group(0);
    let mut child = cmd.spawn().context("starting gamescope")?;

    // gamescope's output goes to the log, and the line saying where its stream is comes here.
    let (node_tx, node_rx) = mpsc::channel();
    let stderr = child.stderr.take().unwrap();
    std::thread::Builder::new().name("gamescope-log".into()).spawn(move || {
        let mut out = out;
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = writeln!(out, "{line}");
            if let Some(id) = line.split(READY).nth(1).and_then(|id| id.trim().parse::<u32>().ok()) {
                let _ = node_tx.send(id);
            }
        }
    })?;

    let thread = std::thread::Builder::new().name("capture".into()).spawn(move || {
        let started = Instant::now();
        let node = loop {
            match node_rx.recv_timeout(CHECK_EVERY) {
                Ok(id) => break Some(id),
                Err(mpsc::RecvTimeoutError::Disconnected) => break None,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if shared.stopping() || started.elapsed() > START_WAIT {
                break None;
            }
            if let Ok(Some(_)) = child.try_wait() {
                break None;
            }
        };
        let child = Rc::new(RefCell::new(child));
        let why = match node {
            Some(node) => stream(node, &shared, child.clone()).err().map(|e| format!("{e:#}")),
            None if shared.stopping() => None,
            None => Some("gamescope didn't start streaming (its output is in the log)".into()),
        };
        let exited = close(&mut child.borrow_mut());
        if !shared.stopping() {
            shared.end(why.unwrap_or_else(|| exited.unwrap_or_else(|| "The game exited".into())));
        }
    })?;
    Ok(thread)
}

/// Stops gamescope and everything in it, if it's still running: asks first, then kills. Says how
/// it ended, if it had ended by itself badly.
fn close(child: &mut Child) -> Option<String> {
    if let Ok(Some(status)) = child.try_wait() {
        return (!status.success()).then(|| format!("The game exited ({status})"));
    }
    let group = -(child.id() as libc::pid_t);
    unsafe { libc::kill(group, libc::SIGTERM) };
    let deadline = Instant::now() + CLOSE_WAIT;
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return None;
        }
        std::thread::sleep(CHECK_EVERY);
    }
    unsafe { libc::kill(group, libc::SIGKILL) };
    let _ = child.wait();
    None
}

/// What the stream's callbacks keep.
struct Taking {
    shared: Arc<Shared>,
    format: spa::param::video::VideoInfoRaw,
}

/// Reads PipeWire node `node`'s video into `shared` until the game's stopped or gamescope exits.
fn stream(node: u32, shared: &Arc<Shared>, child: Rc<RefCell<Child>>) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).context("PipeWire")?;
    let context = pw::context::ContextRc::new(&mainloop, None).context("PipeWire")?;
    let core = context.connect_rc(None).context("connecting to PipeWire")?;
    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Game",
    };
    let stream = pw::stream::StreamBox::new(&core, "romi", props)?;
    let failed: Rc<RefCell<Option<String>>> = Rc::default();
    let _listener = stream
        .add_local_listener_with_user_data(Taking { shared: shared.clone(), format: Default::default() })
        .state_changed({
            let (failed, mainloop) = (failed.clone(), mainloop.clone());
            move |_, _, _, new| {
                if let pw::stream::StreamState::Error(e) = new {
                    *failed.borrow_mut() = Some(format!("gamescope's stream: {e}"));
                    mainloop.quit();
                }
            }
        })
        .param_changed(|_, taking, id, param| {
            if let Some(param) = param
                && id == pw::spa::param::ParamType::Format.as_raw()
            {
                let _ = taking.format.parse(param);
            }
        })
        .process(|stream, taking| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let Some(data) = buffer.datas_mut().first_mut() else { return };
            let (offset, stride) = (data.chunk().offset() as usize, data.chunk().stride() as usize);
            let size = taking.format.size();
            let order = taking.format.format();
            let Some(pixels) = data.data() else { return };
            if let Some(frame) =
                to_frame(pixels.get(offset..).unwrap_or_default(), size.width, size.height, stride, order)
            {
                taking.shared.put_taken_frame(frame);
            }
        })
        .register()?;

    let format = format_pod();
    let mut params = [spa::pod::Pod::from_bytes(&format).unwrap()];
    stream.connect(
        spa::utils::Direction::Input,
        Some(node),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    let check = mainloop.loop_().add_timer({
        let (mainloop, shared) = (mainloop.clone(), shared.clone());
        move |_| {
            let exited = matches!(child.borrow_mut().try_wait(), Ok(Some(_)));
            if shared.stopping() || exited {
                mainloop.quit();
            }
        }
    });
    check.update_timer(Some(CHECK_EVERY), Some(CHECK_EVERY)).into_result()?;
    mainloop.run();
    match failed.take() {
        Some(e) => bail!(e),
        None => Ok(()),
    }
}

/// The formats romi takes the stream in: four bytes a pixel, in either order.
fn format_pod() -> Vec<u8> {
    use pw::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use pw::spa::param::video::VideoFormat;
    let (w, h) = SIZE;
    let obj = pw::spa::pod::object!(
        pw::spa::utils::SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        pw::spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pw::spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pw::spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle { width: w, height: h },
            pw::spa::utils::Rectangle { width: 1, height: 1 },
            pw::spa::utils::Rectangle { width: 8192, height: 8192 }
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: RATE, denom: 1 },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction { num: 1000, denom: 1 }
        ),
    );
    pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .unwrap()
    .0
    .into_inner()
}

/// A frame of the stream as packed RGB: `w`×`h` pixels of four bytes, `stride` bytes a row, in
/// `order`. None for a size or order it can't be.
fn to_frame(pixels: &[u8], w: u32, h: u32, stride: usize, order: spa::param::video::VideoFormat) -> Option<Frame> {
    use spa::param::video::VideoFormat;
    let blue_first = match order {
        VideoFormat::BGRx | VideoFormat::BGRA => true,
        VideoFormat::RGBx | VideoFormat::RGBA => false,
        _ => return None,
    };
    let (wu, hu) = (w as usize, h as usize);
    if wu == 0 || hu == 0 || stride < wu * 4 || pixels.len() < stride * (hu - 1) + wu * 4 {
        return None;
    }
    let mut rgb = Vec::with_capacity(wu * hu * 3);
    for row in pixels.chunks(stride).take(hu) {
        for p in row[..wu * 4].as_chunks::<4>().0 {
            match blue_first {
                true => rgb.extend_from_slice(&[p[2], p[1], p[0]]),
                false => rgb.extend_from_slice(&p[..3]),
            }
        }
    }
    Some(Frame { width: w, height: h, rgb, pixel_aspect: 1.0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use spa::param::video::VideoFormat;

    #[test]
    fn a_frame_is_its_rows_as_rgb_whatever_their_order_and_padding() {
        // Two by two, eight bytes of padding on each row.
        let mut px = Vec::new();
        for row in [[[1, 2, 3, 0], [4, 5, 6, 0]], [[7, 8, 9, 0], [10, 11, 12, 0]]] {
            px.extend(row.concat());
            px.extend([0xEE; 8]);
        }
        let bgr = to_frame(&px, 2, 2, 16, VideoFormat::BGRx).unwrap();
        assert_eq!(bgr.rgb, [3, 2, 1, 6, 5, 4, 9, 8, 7, 12, 11, 10]);
        let rgb = to_frame(&px, 2, 2, 16, VideoFormat::RGBA).unwrap();
        assert_eq!(rgb.rgb, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    }

    #[test]
    fn a_frame_too_short_for_its_size_or_in_another_format_is_none() {
        let px = [0u8; 16];
        assert!(to_frame(&px, 2, 3, 8, VideoFormat::BGRx).is_none());
        assert!(to_frame(&px, 2, 2, 8, VideoFormat::NV12).is_none());
        assert!(to_frame(&px, 2, 2, 4, VideoFormat::BGRx).is_none());
    }
}
