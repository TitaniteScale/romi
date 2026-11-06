//! Games with no libretro core, played in the middle panel anyway: the game's own command runs in
//! gamescope with no screen (its headless backend), gamescope streams what it draws over PipeWire,
//! and each frame of that stream becomes the newest frame in `Shared`, for the panel to show as it
//! shows a core's.
//!
//! romi starts that gamescope itself, except for a Steam game: Steam starts those, so the
//! gamescope has to come from the game's launch options in Steam. They run a script of romi's
//! (`steam_script`), which puts the game in gamescope when romi has just asked for it, and
//! otherwise starts it as usual.
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
use std::path::{Path, PathBuf};
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
/// How long Steam gets to start a game, when it's asked to: it may have to start itself first.
const STEAM_WAIT: Duration = Duration::from_secs(60);
/// How often the stream's thread checks whether the game's been stopped or has exited.
const CHECK_EVERY: Duration = Duration::from_millis(50);
/// How long the game gets to close when it's stopped, before it's killed.
const CLOSE_WAIT: Duration = Duration::from_secs(3);
/// What gamescope says when its stream is ready, before the PipeWire node's id.
const READY: &str = "stream available on node ID: ";

/// What plays in gamescope.
pub enum Program {
    /// A command, which romi runs in a gamescope it starts.
    Command(Prepared),
    /// Steam game `appid`, started by its `launch` command (`steam steam://rungameid/…`), whose
    /// launch options in Steam run it in a gamescope (see `steam_script`). They're set up the first
    /// time, in Steam's folder, if it's known.
    Steam { appid: String, name: String, launch: Prepared, steam_dir: Option<PathBuf> },
}

/// Whether gamescope is installed, which playing this way needs.
pub fn available() -> bool {
    crate::launch::find_program("gamescope").is_some()
}

/// The gamescope a game plays in.
enum Gamescope {
    /// One romi started, in a process group of its own.
    Own(Child),
    /// One the script in a Steam game's launch options started: the files romi and the script
    /// talk through, and gamescope's process, once the script has said which it is.
    Steam { request: PathBuf, pid: Option<libc::pid_t> },
}

impl Gamescope {
    /// Whether it's still running.
    fn running(&mut self) -> bool {
        match self {
            Gamescope::Own(child) => matches!(child.try_wait(), Ok(None)),
            Gamescope::Steam { pid: Some(pid), .. } => alive(*pid),
            Gamescope::Steam { pid: None, .. } => true,
        }
    }

    /// Stops it and everything in it, if it's still running: asks first, then kills. Says how
    /// it ended, if it had ended by itself badly.
    fn close(&mut self) -> Option<String> {
        match self {
            Gamescope::Own(child) => {
                if let Ok(Some(status)) = child.try_wait() {
                    return (!status.success()).then(|| format!("The game exited ({status})"));
                }
                let group = -(child.id() as libc::pid_t);
                stop(group, || matches!(child.try_wait(), Ok(Some(_))));
                let _ = child.wait();
                None
            }
            Gamescope::Steam { request, pid } => {
                // Not asked for any more, if the script never ran.
                let _ = std::fs::remove_file(&*request);
                let _ = std::fs::remove_file(request.with_extension("pid"));
                // Only gamescope itself: its process group is Steam's. Steam sees the game end
                // when it does.
                if let Some(pid) = *pid {
                    stop(pid, || !alive(pid));
                }
                None
            }
        }
    }
}

/// Sends `SIGTERM` to `target` (a process, or a group as minus its id), waits for `ended` for up to
/// `CLOSE_WAIT`, then sends `SIGKILL`.
fn stop(target: libc::pid_t, mut ended: impl FnMut() -> bool) {
    unsafe { libc::kill(target, libc::SIGTERM) };
    let deadline = Instant::now() + CLOSE_WAIT;
    while Instant::now() < deadline {
        if ended() {
            return;
        }
        std::thread::sleep(CHECK_EVERY);
    }
    unsafe { libc::kill(target, libc::SIGKILL) };
}

/// Whether process `pid` is running. Not whether it's gamescope: it's the script until the script
/// starts gamescope in its place, and romi may look before then.
fn alive(pid: libc::pid_t) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return false };
    // "pid (name) state ...": a zombie (Z) has exited.
    stat.rsplit_once(") ").is_some_and(|(_, rest)| !rest.starts_with('Z'))
}

/// The arguments that start gamescope with no screen, before the game's command.
fn gamescope_args() -> Vec<String> {
    let (w, h) = SIZE;
    let size = |n: u32| n.to_string();
    let mut argv: Vec<String> = ["gamescope", "--backend", "headless", "-w"].map(String::from).into();
    argv.extend([size(w), "-h".into(), size(h), "-W".into(), size(w), "-H".into(), size(h)]);
    argv.extend(["-r".into(), RATE.to_string(), "--".into()]);
    argv
}

/// Starts `program` in a hidden gamescope, with its output in `log`, and a thread that puts each
/// frame it draws in `shared`. When the game exits by itself, `shared` says so
/// (`Shared::take_ended`).
pub fn start(program: Program, log: &Path, shared: Arc<Shared>) -> Result<JoinHandle<()>> {
    if !available() {
        bail!("gamescope isn't installed, and playing this in romi needs it");
    }
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let (gamescope, node, steam) = match program {
        Program::Command(game) => {
            let (gamescope, node) = start_own(&game, log)?;
            (gamescope, node, None)
        }
        Program::Steam { appid, name, launch, steam_dir } => {
            let script = steam_script()?;
            std::fs::File::create(log)?;
            let request = steam_request(&appid);
            let ask = SteamAsk { appid, name, launch, steam_dir, script, request: request.clone(), log: log.into() };
            (Gamescope::Steam { request, pid: None }, Node::InLog(log.to_path_buf()), Some(ask))
        }
    };
    let thread = std::thread::Builder::new().name("capture".into()).spawn(move || {
        let mut gamescope = gamescope;
        // Steam may have to be restarted first, which takes a while, so it's asked here.
        let node = match steam.map(|ask| ask.send()) {
            Some(Err(e)) => Err(e),
            _ => node.wait(&shared, &mut gamescope),
        };
        let gamescope = Rc::new(RefCell::new(gamescope));
        let why = match node {
            Ok(Some(node)) => stream(node, &shared, gamescope.clone()).err().map(|e| format!("{e:#}")),
            Ok(None) => None,
            Err(e) => Some(e),
        };
        let exited = gamescope.borrow_mut().close();
        if !shared.stopping() {
            shared.end(why.unwrap_or_else(|| exited.unwrap_or_else(|| "The game exited".into())));
        }
    })?;
    Ok(thread)
}

/// Where gamescope will say which PipeWire node its stream is.
enum Node {
    /// From its output, read by a thread of romi's.
    Said(mpsc::Receiver<u32>),
    /// In the log the Steam script has it write to.
    InLog(PathBuf),
}

impl Node {
    /// Waits for gamescope to start streaming, and says the stream's node. None if the game was
    /// stopped first.
    fn wait(self, shared: &Shared, gamescope: &mut Gamescope) -> Result<Option<u32>, String> {
        let asked = Instant::now();
        // When the Steam script said it was starting gamescope.
        let mut started = None;
        loop {
            let node = match &self {
                Node::Said(rx) => match rx.recv_timeout(CHECK_EVERY) {
                    Ok(id) => Some(id),
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Err(not_streaming()),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                },
                Node::InLog(log) => {
                    std::thread::sleep(CHECK_EVERY);
                    std::fs::read_to_string(log).ok().as_deref().and_then(node_in)
                }
            };
            if let Some(node) = node {
                return Ok(Some(node));
            }
            if shared.stopping() {
                return Ok(None);
            }
            if let Gamescope::Steam { request, pid } = gamescope
                && pid.is_none()
            {
                *pid = std::fs::read_to_string(request.with_extension("pid")).ok().and_then(|p| p.trim().parse().ok());
                if pid.is_none() && asked.elapsed() > STEAM_WAIT {
                    return Err("Steam didn't start the game (is Steam signed in?)".into());
                }
                if pid.is_some() {
                    started = Some(Instant::now());
                }
                continue;
            }
            if started.unwrap_or(asked).elapsed() > START_WAIT || !gamescope.running() {
                return Err(not_streaming());
            }
        }
    }
}

fn not_streaming() -> String {
    "gamescope didn't start streaming (its output is in the log)".into()
}

/// The stream's node, from gamescope's output.
fn node_in(output: &str) -> Option<u32> {
    output.lines().find_map(|line| line.split(READY).nth(1)?.trim().parse().ok())
}

/// Starts `game` in a gamescope of romi's, which says where its stream is on its error output.
fn start_own(game: &Prepared, log: &Path) -> Result<(Gamescope, Node)> {
    let mut argv = gamescope_args();
    argv.extend(game.argv.iter().cloned());
    let wrapped = Prepared { argv, cwd: game.cwd.clone(), env: game.env.clone() };

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
            if let Some(id) = node_in(&line) {
                let _ = node_tx.send(id);
            }
        }
    })?;
    Ok((Gamescope::Own(child), Node::Said(node_rx)))
}

/// A Steam game to ask Steam for (see `Program::Steam`), with the script its launch options
/// should run, and the request for it and the log to leave the script.
struct SteamAsk {
    appid: String,
    name: String,
    launch: Prepared,
    steam_dir: Option<PathBuf>,
    script: PathBuf,
    request: PathBuf,
    log: PathBuf,
}

impl SteamAsk {
    /// Sets Steam's games up to play in romi, if this one isn't, leaves word for the script in its
    /// launch options to start it in gamescope, with gamescope's output in the log, and asks Steam
    /// for it.
    fn send(self) -> Result<(), String> {
        if let Some(dir) = &self.steam_dir {
            self.set_up(dir)?;
        }
        let failed = |e: std::io::Error| format!("asking Steam for {}: {e}", self.name);
        if let Some(dir) = self.request.parent() {
            std::fs::create_dir_all(dir).map_err(failed)?;
        }
        let _ = std::fs::remove_file(self.request.with_extension("pid"));
        std::fs::write(&self.request, self.log.as_os_str().as_encoded_bytes()).map_err(failed)?;

        crate::steam::start_client_quietly(&self.launch.argv);
        let mut cmd = self.launch.command();
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
        match cmd.spawn() {
            // It hands the game to the Steam client and exits.
            Ok(mut asked) => drop(std::thread::spawn(move || asked.wait())),
            Err(e) => {
                let _ = std::fs::remove_file(&self.request);
                return Err(failed(e));
            }
        }
        Ok(())
    }

    /// If the game's launch options don't run the script, sets every installed game's to, so the
    /// next game needs nothing either. Steam has to be closed for that, so a running Steam is
    /// closed, unless it's running a game, and `send` starts it again, with no window.
    fn set_up(&self, dir: &Path) -> Result<(), String> {
        let ready = || crate::steam::launch_options(dir, &self.appid).iter().any(|o| o.contains(SCRIPT_NAME));
        if ready() {
            return Ok(());
        }
        let name = &self.name;
        if crate::steam::client_running() {
            if crate::steam::game_running() {
                return Err(format!(
                    "To play {name} in romi, romi has to restart Steam once to set it up, and a Steam game is running"
                ));
            }
            if !crate::steam::shut_down_client() {
                return Err(format!("Steam didn't close, and romi has to restart it once to set {name} up"));
            }
        }
        let mut appids: Vec<String> = crate::steam::installed_apps(dir, &[]).into_iter().map(|a| a.appid).collect();
        if !appids.contains(&self.appid) {
            appids.push(self.appid.clone());
        }
        crate::steam::wrap_launch_options(dir, &appids, &self.script)
            .map_err(|e| format!("setting up Steam's games to play in romi: {e:#}"))?;
        if !ready() {
            return Err(format!(
                "romi couldn't set {name}'s launch options: set them in Steam (Properties) to \"{}\" %command%",
                self.script.display()
            ));
        }
        Ok(())
    }
}

/// The name of the script a Steam game's launch options run, for romi to play it.
const SCRIPT_NAME: &str = "steam-gamescope";

/// Writes the script a Steam game's launch options run (`"<script>" %command%`) for romi to play
/// it, if it isn't there as it should be, and says where it is. When romi has just asked for the
/// game, in the file `steam_request` names, it starts the game in a hidden gamescope, says which
/// process that is, and has gamescope write to the log the request names. Otherwise, as when the
/// game's started in Steam itself, it starts the game as usual.
fn steam_script() -> Result<PathBuf> {
    let path = crate::config::data_dir().join(SCRIPT_NAME);
    let request = steam_request("$SteamAppId");
    let gamescope = shell_words::join(gamescope_args());
    let script = format!(
        "#!/bin/sh\n\
         # romi writes this, for a Steam game's launch options: \"{path}\" %command%\n\
         # The game plays in romi when romi asks for it, and as usual when it doesn't.\n\
         request=\"{request}\"\n\
         if [ -n \"$SteamAppId\" ] && [ -n \"$(find \"$request\" -mmin -2 2>/dev/null)\" ]; then\n\
         \tlog=$(cat \"$request\")\n\
         \trm -f \"$request\"\n\
         \techo $$ > \"$request.tmp\" && mv \"$request.tmp\" \"$request.pid\"\n\
         \texec {gamescope} \"$@\" >> \"$log\" 2>&1\n\
         fi\n\
         exec \"$@\"\n",
        path = path.display(),
        request = request.display(),
    );
    if std::fs::read_to_string(&path).ok().as_deref() != Some(&script) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, &script).with_context(|| format!("writing {}", path.display()))?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(path)
}

/// The file that asks the script in Steam game `appid`'s launch options to start it in gamescope.
/// The script writes gamescope's process id next to it, with the extension `pid`.
fn steam_request(appid: &str) -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    runtime.join("romi").join(format!("steam-{appid}"))
}

/// What the stream's callbacks keep.
struct Taking {
    shared: Arc<Shared>,
    format: spa::param::video::VideoInfoRaw,
}

/// Reads PipeWire node `node`'s video into `shared` until the game's stopped or gamescope exits.
fn stream(node: u32, shared: &Arc<Shared>, gamescope: Rc<RefCell<Gamescope>>) -> Result<()> {
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
            if shared.stopping() || !gamescope.borrow_mut().running() {
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
