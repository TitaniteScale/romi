//! RetroAchievements in games played inside romi, through rcheevos (RetroAchievements' own C
//! library, in `vendor/rcheevos`) and romi's glue to it (`cheevos.c`).
//!
//! Each game played in romi gets a `Client` on the thread running its core. It logs in with the
//! token `romi achievements login` saved, loads the game's achievements by the ROM's hash, and
//! checks them against the game's memory after every frame. Unlocks are sent to the user's
//! account as they happen, softcore: the server turns hardcore unlocks from clients it doesn't
//! recognize into softcore ones.
//!
//! rcheevos makes its HTTP requests through romi, which runs each on a thread of its own so a
//! slow server never holds up a frame. A client is only freed once every request it made has been
//! answered, since the answer calls back into it.

use crate::emu::libretro::{Core, MemoryDataFn, MemoryMap, MemorySizeFn};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

// ---- cheevos.c ----

/// rcheevos' `rc_client_t`, only ever handled by pointer.
#[repr(C)]
struct RcClient {
    _private: [u8; 0],
}

/// `romi_rc_event` in cheevos.c.
#[repr(C)]
struct RawEvent {
    kind: u32,
    id: u32,
    points: u32,
    title: *const c_char,
    description: *const c_char,
    badge_url: *const c_char,
    progress: *const c_char,
    error: *const c_char,
}

/// rcheevos' callback for answering a request, passed back to `romi_rc_respond` as it came.
type Respond = *const c_void;

/// `romi_rc_callbacks` in cheevos.c.
#[repr(C)]
struct Callbacks {
    http: extern "C" fn(*mut c_void, *const c_char, *const c_char, *const c_char, Respond, *mut c_void),
    event: extern "C" fn(*mut c_void, *const RawEvent),
    done: extern "C" fn(*mut c_void, c_int, c_int, *const c_char),
    log: extern "C" fn(*mut c_void, *const c_char),
}

unsafe extern "C" {
    fn romi_rc_create(callbacks: *const Callbacks, rust: *mut c_void, host: *const c_char) -> *mut RcClient;
    fn romi_rc_destroy(client: *mut RcClient);
    fn romi_rc_respond(respond: Respond, data: *mut c_void, body: *const c_char, len: usize, status: c_int);
    fn romi_rc_memory_init(
        client: *mut RcClient,
        map: *const MemoryMap,
        console: u32,
        data: MemoryDataFn,
        size: MemorySizeFn,
    ) -> c_int;
    fn romi_rc_login_with_password(client: *mut RcClient, username: *const c_char, password: *const c_char);
    fn romi_rc_login_with_token(client: *mut RcClient, username: *const c_char, token: *const c_char);
    fn romi_rc_load_game(client: *mut RcClient, hash: *const c_char);
    fn romi_rc_user(client: *const RcClient, username: *mut *const c_char, token: *mut *const c_char);
    fn romi_rc_game(
        client: *mut RcClient,
        unlocked: *mut u32,
        total: *mut u32,
        points_unlocked: *mut u32,
        points_total: *mut u32,
    ) -> *const c_char;
    fn romi_rc_user_agent_clause(client: *mut RcClient, buffer: *mut c_char, size: usize) -> usize;
    fn rc_client_do_frame(client: *mut RcClient);
    fn rc_client_idle(client: *mut RcClient);
}

const DONE_LOGIN: c_int = 1;
const DONE_LOAD: c_int = 2;
const RC_OK: c_int = 0;
/// What rcheevos is told when a request didn't reach the server, so it tries again.
const RETRYABLE_CLIENT_ERROR: c_int = -2;

const EVENT_ACHIEVEMENT_TRIGGERED: u32 = 1;
const EVENT_GAME_COMPLETED: u32 = 15;
const EVENT_SERVER_ERROR: u32 = 16;
const EVENT_DISCONNECTED: u32 = 17;
const EVENT_RECONNECTED: u32 = 18;

/// A RetroAchievements server other than the real one, for tests (`http://127.0.0.1:8080`).
const HOST_VAR: &str = "ROMI_RA_HOST";

// ---- what happens ----

/// Something to tell the player while they play.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The game's achievements are loaded, with how many are already unlocked.
    Loaded {
        title: String,
        unlocked: u32,
        total: u32,
        points: u32,
        points_total: u32,
    },
    Unlocked {
        title: String,
        description: String,
        points: u32,
        badge_url: Option<String>,
    },
    /// Every achievement in the game is unlocked.
    Mastered {
        title: String,
    },
    /// Unlocks couldn't be sent and are waiting, or they've gone through after all.
    Offline,
    Online,
    /// Something went wrong: the login, an unknown game, the server.
    Problem(String),
}

/// What the client and the callbacks it makes share.
struct Inner {
    events: Mutex<Vec<Event>>,
    /// Finished logins and loads: what, and the error if it failed.
    done: Mutex<Vec<(c_int, Option<String>)>>,
    /// The game's achievements are loaded, and their summary is to be told once the core thread
    /// next has the client.
    loaded: AtomicBool,
    in_flight: AtomicUsize,
    agent: ureq::Agent,
    user_agent: OnceLock<String>,
}

fn text(p: *const c_char) -> Option<String> {
    // SAFETY: rcheevos' strings are NUL-terminated and live until the callback returns.
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()).filter(|s| !s.is_empty())
}

/// The `Inner` behind a callback's pointer.
fn inner<'a>(rust: *mut c_void) -> &'a Inner {
    // SAFETY: the pointer is the client's `Inner`, kept alive for as long as rcheevos may call back.
    unsafe { &*(rust as *const Inner) }
}

extern "C" fn on_http(
    rust: *mut c_void,
    url: *const c_char,
    post: *const c_char,
    content_type: *const c_char,
    respond: Respond,
    respond_data: *mut c_void,
) {
    let inner_ptr = rust as usize;
    let (url, post, content_type) = (text(url).unwrap_or_default(), text(post), text(content_type));
    let (respond, respond_data) = (respond as usize, respond_data as usize);
    let me = inner(rust);
    me.in_flight.fetch_add(1, Ordering::SeqCst);
    let agent = me.agent.clone();
    let user_agent = me.user_agent.get().cloned().unwrap_or_else(|| user_agent(""));
    std::thread::spawn(move || {
        // rcheevos tries unlocks again itself, but not logins or loads, so a request that never got
        // an answer is tried again here first. Asking twice is harmless: the server takes an
        // unlock it already has as done.
        let mut tries = RETRY_AFTER.iter();
        let (status, body) = loop {
            match request(&agent, &user_agent, &url, post.as_deref(), content_type.as_deref()) {
                Ok(answer) => break answer,
                Err(e) => {
                    log(&format!("request failed: {e:#}"));
                    match tries.next() {
                        Some(wait) => std::thread::sleep(*wait),
                        None => break (RETRYABLE_CLIENT_ERROR, Vec::new()),
                    }
                }
            }
        };
        // SAFETY: rcheevos' callback and its data, as it gave them, answered exactly once.
        unsafe {
            romi_rc_respond(respond as Respond, respond_data as *mut c_void, body.as_ptr().cast(), body.len(), status);
        }
        inner(inner_ptr as *mut c_void).in_flight.fetch_sub(1, Ordering::SeqCst);
    });
}

/// Sends a request, with its status and body back whatever the status.
fn request(
    agent: &ureq::Agent,
    user_agent: &str,
    url: &str,
    post: Option<&str>,
    content_type: Option<&str>,
) -> Result<(c_int, Vec<u8>)> {
    let mut response = match post {
        Some(body) => agent
            .post(url)
            .header("User-Agent", user_agent)
            .header("Content-Type", content_type.unwrap_or("application/x-www-form-urlencoded"))
            .send(body)?,
        None => agent.get(url).header("User-Agent", user_agent).call()?,
    };
    let status = response.status().as_u16() as c_int;
    let body = response.body_mut().with_config().limit(32 << 20).read_to_vec()?;
    Ok((status, body))
}

extern "C" fn on_event(rust: *mut c_void, raw: *const RawEvent) {
    // SAFETY: cheevos.c passes an event that lives until this returns.
    let raw = unsafe { &*raw };
    let event = match raw.kind {
        EVENT_ACHIEVEMENT_TRIGGERED => Event::Unlocked {
            title: text(raw.title).unwrap_or_default(),
            description: text(raw.description).unwrap_or_default(),
            points: raw.points,
            badge_url: text(raw.badge_url),
        },
        EVENT_GAME_COMPLETED => Event::Mastered { title: text(raw.title).unwrap_or_default() },
        EVENT_SERVER_ERROR => Event::Problem(format!(
            "RetroAchievements: {}",
            text(raw.error).unwrap_or_else(|| "the server had a problem".into())
        )),
        EVENT_DISCONNECTED => Event::Offline,
        EVENT_RECONNECTED => Event::Online,
        _ => return,
    };
    inner(rust).events.lock().unwrap_or_else(|e| e.into_inner()).push(event);
}

extern "C" fn on_done(rust: *mut c_void, what: c_int, result: c_int, error: *const c_char) {
    let me = inner(rust);
    let error = (result != RC_OK).then(|| text(error).unwrap_or_else(|| format!("error {result}")));
    if what == DONE_LOAD && error.is_none() {
        me.loaded.store(true, Ordering::SeqCst);
    }
    me.done.lock().unwrap_or_else(|e| e.into_inner()).push((what, error));
}

/// rcheevos' warnings go to the file named by this, when it's set; the TUI has nowhere for them.
const LOG_VAR: &str = "ROMI_RA_LOG";

/// How long to wait before trying a request again that never got an answer, each time.
const RETRY_AFTER: [Duration; 2] = [Duration::from_millis(500), Duration::from_secs(1)];

fn log(message: &str) {
    use std::io::Write;
    let Ok(path) = std::env::var(LOG_VAR) else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
    }
}

extern "C" fn on_log(_rust: *mut c_void, message: *const c_char) {
    if let Some(message) = text(message) {
        log(&message);
    }
}

static CALLBACKS: Callbacks = Callbacks { http: on_http, event: on_event, done: on_done, log: on_log };

fn user_agent(clause: &str) -> String {
    format!("romi/{} {clause}", env!("CARGO_PKG_VERSION")).trim().to_string()
}

// ---- a client ----

pub struct Client {
    raw: *mut RcClient,
    inner: *const Inner,
    /// RetroAchievements' console id for the game, whose memory is laid out that way.
    console: u32,
    /// Which of the core's memory maps the memory was last laid out from (see
    /// `Core::memory_map_given`), and whether any of it could be found.
    laid_out: Option<(u64, bool)>,
}

// SAFETY: rcheevos' client guards itself with a lock, and `Inner` is all locks and atomics.
unsafe impl Send for Client {}

fn c_string(s: &str) -> Result<CString> {
    CString::new(s).context("has a NUL in it")
}

impl Client {
    /// A client for a game on `console` (RetroAchievements' id for it; 0 when there's no game).
    pub fn new(console: u32) -> Result<Client> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .into();
        let inner = Arc::into_raw(Arc::new(Inner {
            events: Mutex::new(Vec::new()),
            done: Mutex::new(Vec::new()),
            loaded: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            agent,
            user_agent: OnceLock::new(),
        }));
        let host = std::env::var(HOST_VAR).ok().map(|h| c_string(&h)).transpose()?;
        // SAFETY: the callbacks are 'static and `inner` outlives the client (see Drop).
        let raw = unsafe {
            romi_rc_create(&CALLBACKS, inner as *mut c_void, host.as_ref().map_or(std::ptr::null(), |h| h.as_ptr()))
        };
        if raw.is_null() {
            // SAFETY: from Arc::into_raw above, and nothing else has it.
            drop(unsafe { Arc::from_raw(inner) });
            bail!("couldn't start rcheevos");
        }
        let client = Client { raw, inner, console, laid_out: None };
        let mut clause = [0 as c_char; 128];
        // SAFETY: the buffer is as big as it's said to be.
        unsafe { romi_rc_user_agent_clause(raw, clause.as_mut_ptr(), clause.len()) };
        let _ = client.inner().user_agent.set(user_agent(&text(clause.as_ptr()).unwrap_or_default()));
        Ok(client)
    }

    fn inner(&self) -> &Inner {
        // SAFETY: kept alive until Drop.
        unsafe { &*self.inner }
    }

    /// Lays out the game's memory as RetroAchievements numbers it, from the core's memory map if
    /// it has given one and its plain memory if not, unless that's already been done for this map.
    /// Returns whether any could be found.
    fn lay_out_memory(&mut self, core: &Core) -> bool {
        if let Some((given, found)) = self.laid_out
            && given == core.memory_map_given()
        {
            return found;
        }
        let map = core.memory_map();
        let raw_map = MemoryMap { descriptors: map.0.as_ptr(), num_descriptors: map.0.len() as c_uint };
        let (data, size) = core.memory_functions();
        let map_ptr = if map.0.is_empty() { std::ptr::null() } else { &raw_map as *const MemoryMap };
        // SAFETY: rcheevos copies what it needs of the map while this runs; the memory it points
        // into is the core's, which outlives the client's reads (they only happen in `frame`).
        let found = unsafe { romi_rc_memory_init(self.raw, map_ptr, self.console, data, size) != 0 };
        if !found && self.laid_out.is_none_or(|(_, found_before)| found_before) {
            let problem = "RetroAchievements: can't find this game's memory in the core".to_string();
            self.inner().events.lock().unwrap_or_else(|e| e.into_inner()).push(Event::Problem(problem));
        }
        self.laid_out = Some((map.1, found));
        found
    }

    pub fn login_with_token(&self, login: &Login) -> Result<()> {
        let (user, token) = (c_string(&login.username)?, c_string(&login.token)?);
        // SAFETY: rcheevos copies the strings.
        unsafe { romi_rc_login_with_token(self.raw, user.as_ptr(), token.as_ptr()) };
        Ok(())
    }

    /// Loads the game with this RetroAchievements hash; rcheevos waits for the login first.
    pub fn load_game(&self, hash: &str) -> Result<()> {
        let hash = c_string(hash)?;
        // SAFETY: rcheevos copies the string.
        unsafe { romi_rc_load_game(self.raw, hash.as_ptr()) };
        Ok(())
    }

    /// Checks the achievements against this frame's memory. Call after each frame `core` runs.
    /// Memory is laid out here, after a frame, because some cores (mGBA) only give their memory
    /// map once they've run one; rcheevos checks achievements' addresses as it starts them, which
    /// it only does in `frame` and `idle`.
    pub fn frame(&mut self, core: &Core) {
        self.lay_out_memory(core);
        // SAFETY: the client is alive, and this is the thread running the core.
        unsafe { rc_client_do_frame(self.raw) };
        self.after();
    }

    /// Keeps things moving (retries, pings) while the game is paused. Only once it has run a
    /// frame (see `frame`).
    pub fn idle(&mut self) {
        if self.laid_out.is_none() {
            return;
        }
        // SAFETY: as in `frame`.
        unsafe { rc_client_idle(self.raw) };
        self.after();
    }

    /// Turns logins and loads that finished into what to tell the player.
    fn after(&mut self) {
        let done: Vec<_> = std::mem::take(&mut *self.inner().done.lock().unwrap_or_else(|e| e.into_inner()));
        let mut events = Vec::new();
        let mut login_failed = false;
        for (what, error) in done {
            match (what, error) {
                (DONE_LOGIN, Some(e)) => {
                    login_failed = true;
                    events.push(Event::Problem(format!(
                        "RetroAchievements login failed ({e}); log in again with `romi achievements login`"
                    )));
                }
                // A load that waited on a failed login says so again; once is enough.
                (DONE_LOAD, Some(_)) if login_failed => {}
                (DONE_LOAD, Some(e)) => events.push(Event::Problem(format!("RetroAchievements: {e}"))),
                _ => {}
            }
        }
        if self.inner().loaded.swap(false, Ordering::SeqCst) {
            let (mut unlocked, mut total, mut points, mut points_total) = (0, 0, 0, 0);
            // SAFETY: the client is alive; the title lives until the game is unloaded.
            let title = unsafe { romi_rc_game(self.raw, &mut unlocked, &mut total, &mut points, &mut points_total) };
            events.push(Event::Loaded {
                title: text(title).unwrap_or_default(),
                unlocked,
                total,
                points,
                points_total,
            });
        }
        self.inner().events.lock().unwrap_or_else(|e| e.into_inner()).extend(events);
    }

    /// What's happened since the last time, oldest first.
    pub fn take_events(&self) -> Vec<Event> {
        std::mem::take(&mut *self.inner().events.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Logs in with a password, waiting up to `wait`, and gives back the login to keep.
    pub fn login_with_password(&self, username: &str, password: &str, wait: Duration) -> Result<Login> {
        let (user, pass) = (c_string(username)?, c_string(password)?);
        // SAFETY: rcheevos copies the strings.
        unsafe { romi_rc_login_with_password(self.raw, user.as_ptr(), pass.as_ptr()) };
        let started = Instant::now();
        loop {
            let done = self.inner().done.lock().unwrap_or_else(|e| e.into_inner()).pop();
            match done {
                Some((DONE_LOGIN, None)) => break,
                Some((DONE_LOGIN, Some(e))) => bail!("{e}"),
                _ if started.elapsed() > wait => bail!("retroachievements.org didn't answer"),
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        let (mut user, mut token) = (std::ptr::null(), std::ptr::null());
        // SAFETY: the client is alive; the strings are copied before anything else touches it.
        unsafe { romi_rc_user(self.raw, &mut user, &mut token) };
        match (text(user), text(token)) {
            (Some(username), Some(token)) => Ok(Login { username, token }),
            _ => bail!("logged in, but retroachievements.org gave no token"),
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Every request must be answered before the client goes, since the answer calls into it.
        let started = Instant::now();
        while self.inner().in_flight.load(Ordering::SeqCst) > 0 && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.inner().in_flight.load(Ordering::SeqCst) > 0 {
            // Still waiting on the server: leaving the client be is safer than freeing it under
            // a reply. It's small, and this only happens when romi stops a game mid-request.
            return;
        }
        // SAFETY: nothing calls back into the client any more, and `inner` came from
        // Arc::into_raw in `new`.
        unsafe {
            romi_rc_destroy(self.raw);
            drop(Arc::from_raw(self.inner));
        }
    }
}

// ---- the saved login ----

/// A RetroAchievements login: the username and the token the server gave for the password, which
/// romi keeps instead of the password.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Login {
    pub username: String,
    pub token: String,
}

/// Where the login is kept: the state folder, which isn't shared the way a config can be.
pub fn login_path() -> PathBuf {
    crate::config::state_dir().join("retroachievements-login.json")
}

pub fn saved_login() -> Option<Login> {
    serde_json::from_slice(&std::fs::read(login_path()).ok()?).ok()
}

/// Saves the login, readable only by the user.
pub fn save_login(login: &Login) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = login_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(login)?)?;
    file.sync_all()?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Forgets the login. False if there wasn't one.
pub fn forget_login() -> Result<bool> {
    match std::fs::remove_file(login_path()) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// What a game needs for achievements while it's played in romi.
#[derive(Debug, Clone)]
pub struct Setup {
    pub login: Login,
    /// RetroAchievements' console id and the ROM's hash.
    pub console: u32,
    pub hash: String,
}
