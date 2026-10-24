//! Showing box art in the TUI. Decoding and resizing happen on worker threads so scrolling never
//! waits on an image; the UI shows whatever is ready and asks for the rest. Each frame says which
//! covers it wanted, in order (the selected one first); covers that scrolled out of view before a
//! worker got to them are dropped.

use crate::art::{Finder, Kind};
use crate::config::Config;
use crate::library::Library;
use image::imageops::FilterType;
use ratatui::layout::Size;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{FontSize, Resize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};

/// Rendered covers kept around; enough to scroll back and forth through a few screens of the
/// cover grid instantly.
const KEEP: usize = 160;
/// Decoding threads, at most.
const WORKERS: usize = 4;

type Slot = (String, Size);

#[derive(Clone)]
struct Request {
    slot: Slot,
    path: PathBuf,
}

pub enum Cover<'a> {
    Ready(&'a Protocol),
    Loading,
    None,
}

/// What the workers share: covers still to decode, best first, and the ones being decoded now.
#[derive(Default)]
struct Work {
    queue: VecDeque<Request>,
    busy: HashSet<Slot>,
}

pub struct Covers {
    finder: Finder,
    font: FontSize,
    /// Game key and kind -> image on disk (or none), resolved once per run.
    paths: HashMap<(String, Kind), Option<PathBuf>>,
    ready: HashMap<Slot, Protocol>,
    order: VecDeque<Slot>,
    failed: HashSet<Slot>,
    /// Covers the frame being drawn wanted and didn't have, in order.
    wanted: Vec<Request>,
    /// What the workers were last given, so an unchanged frame doesn't disturb them.
    sent: Vec<Slot>,
    work: Arc<(Mutex<Work>, Condvar)>,
    rx: Receiver<(Slot, Option<Protocol>)>,
}

impl Covers {
    pub fn new(picker: Picker, cfg: &Config) -> Covers {
        let (tx, rx) = mpsc::channel();
        let work = Arc::new((Mutex::new(Work::default()), Condvar::new()));
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, WORKERS);
        for i in 0..threads {
            let (picker, tx, work) = (picker.clone(), tx.clone(), Arc::clone(&work));
            std::thread::Builder::new()
                .name(format!("covers-{i}"))
                .spawn(move || {
                    let (lock, ready) = &*work;
                    loop {
                        let req = {
                            let Ok(mut w) = lock.lock() else { return };
                            loop {
                                if let Some(req) = w.queue.pop_front() {
                                    w.busy.insert(req.slot.clone());
                                    break req;
                                }
                                let Ok(next) = ready.wait(w) else { return };
                                w = next;
                            }
                        };
                        let proto = encode(&picker, &req);
                        if let Ok(mut w) = lock.lock() {
                            w.busy.remove(&req.slot);
                        }
                        if tx.send((req.slot, proto)).is_err() {
                            return;
                        }
                    }
                })
                .ok();
        }
        Covers {
            finder: Finder::new(cfg),
            font: picker.font_size(),
            paths: HashMap::new(),
            ready: HashMap::new(),
            order: VecDeque::new(),
            failed: HashSet::new(),
            wanted: Vec::new(),
            sent: Vec::new(),
            work,
            rx,
        }
    }

    /// The terminal's character cell size in pixels, for sizing covers to their shape.
    pub fn font_size(&self) -> (u16, u16) {
        (self.font.width.max(1), self.font.height.max(1))
    }

    /// Call before drawing a frame: what it asks for replaces what the last one did.
    pub fn begin_frame(&mut self) {
        self.wanted.clear();
    }

    /// The cover image file for a game, if it has one.
    pub fn path(&mut self, lib: &Library, game: usize) -> Option<PathBuf> {
        self.path_kind(lib, game, Kind::Cover)
    }

    pub fn path_kind(&mut self, lib: &Library, game: usize, kind: Kind) -> Option<PathBuf> {
        let g = &lib.games[game];
        let key = (g.key.clone(), kind);
        if let Some(p) = self.paths.get(&key) {
            return p.clone();
        }
        let found = self.finder.find_kind(lib.system_of(game), g, kind).map(|(p, _)| p);
        self.paths.insert(key, found.clone());
        found
    }

    /// The cover for a game sized to fit `size` cells, kicking off a load if it isn't ready.
    pub fn get(&mut self, lib: &Library, game: usize, size: Size) -> Cover<'_> {
        self.get_kind(lib, game, Kind::Cover, size)
    }

    pub fn get_kind(&mut self, lib: &Library, game: usize, kind: Kind, size: Size) -> Cover<'_> {
        if size.width < 4 || size.height < 3 {
            return Cover::None;
        }
        let Some(path) = self.path_kind(lib, game, kind) else { return Cover::None };
        let key = &lib.games[game].key;
        let slot = (if kind == Kind::Cover { key.clone() } else { format!("{key}#{}", kind.plural()) }, size);
        if self.failed.contains(&slot) {
            return Cover::None;
        }
        if !self.ready.contains_key(&slot) {
            if !self.wanted.iter().any(|r| r.slot == slot) {
                self.wanted.push(Request { slot, path });
            }
            return Cover::Loading;
        }
        Cover::Ready(&self.ready[&slot])
    }

    /// Collects finished images, and hands the workers what the last frame wanted. Returns true
    /// if anything new is ready to draw.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok((slot, proto)) = self.rx.try_recv() {
            match proto {
                Some(p) => {
                    self.order.push_back(slot.clone());
                    self.ready.insert(slot, p);
                    while self.order.len() > KEEP {
                        if let Some(old) = self.order.pop_front() {
                            self.ready.remove(&old);
                        }
                    }
                }
                None => {
                    self.failed.insert(slot);
                }
            }
            changed = true;
        }
        let wanted: Vec<Slot> =
            self.wanted.iter().map(|r| r.slot.clone()).filter(|s| !self.ready.contains_key(s)).collect();
        if wanted != self.sent {
            let (lock, ready) = &*self.work;
            if let Ok(mut w) = lock.lock() {
                let busy = std::mem::take(&mut w.busy);
                w.queue = self
                    .wanted
                    .iter()
                    .filter(|r| !busy.contains(&r.slot) && !self.ready.contains_key(&r.slot))
                    .cloned()
                    .collect();
                w.busy = busy;
                ready.notify_all();
            }
            self.sent = wanted;
        }
        changed
    }

    /// Forgets rendered images, e.g. after another program had the terminal: the terminal may
    /// have dropped image data we'd otherwise assume it still has.
    pub fn forget_rendered(&mut self) {
        self.ready.clear();
        self.order.clear();
        self.sent.clear();
    }

    /// Starts over after a config reload (art folders may have changed).
    pub fn reset(&mut self, cfg: &Config) {
        self.finder = Finder::new(cfg);
        self.paths.clear();
        self.failed.clear();
        self.forget_rendered();
    }
}

fn encode(picker: &Picker, req: &Request) -> Option<Protocol> {
    let img = image::ImageReader::open(&req.path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    // Scale rather than Fit, so small images (title screens are 320 pixels wide) fill the space
    // on a high-resolution screen too.
    picker.new_protocol(img, req.slot.1, Resize::Scale(Some(FilterType::Triangle))).ok()
}
