//! Showing box art in the TUI. Decoding and resizing happen on a worker thread so scrolling never
//! waits on an image; the UI shows whatever is ready and asks for the rest.

use crate::art::Finder;
use crate::config::Config;
use crate::library::Library;
use image::imageops::FilterType;
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

/// Rendered covers kept around; enough to scroll back and forth through a screenful instantly.
const KEEP: usize = 48;

type Slot = (String, Size);

struct Request {
    slot: Slot,
    path: PathBuf,
}

pub enum Cover<'a> {
    Ready(&'a Protocol),
    Loading,
    None,
}

pub struct Covers {
    finder: Finder,
    /// Game key -> image on disk (or none), resolved once per run.
    paths: HashMap<String, Option<PathBuf>>,
    ready: HashMap<Slot, Protocol>,
    order: VecDeque<Slot>,
    failed: HashSet<Slot>,
    requested: Option<Slot>,
    tx: Sender<Request>,
    rx: Receiver<(Slot, Option<Protocol>)>,
}

impl Covers {
    pub fn new(picker: Picker, cfg: &Config) -> Covers {
        let (tx, worker_rx) = mpsc::channel::<Request>();
        let (worker_tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("covers".into())
            .spawn(move || {
                while let Ok(mut req) = worker_rx.recv() {
                    // Only the newest request matters; the cursor has moved past the others.
                    while let Ok(newer) = worker_rx.try_recv() {
                        req = newer;
                    }
                    let proto = encode(&picker, &req);
                    if worker_tx.send((req.slot, proto)).is_err() {
                        return;
                    }
                }
            })
            .ok();
        Covers {
            finder: Finder::new(cfg),
            paths: HashMap::new(),
            ready: HashMap::new(),
            order: VecDeque::new(),
            failed: HashSet::new(),
            requested: None,
            tx,
            rx,
        }
    }

    /// The image file for a game, if it has one.
    pub fn path(&mut self, lib: &Library, game: usize) -> Option<PathBuf> {
        let g = &lib.games[game];
        if let Some(p) = self.paths.get(&g.key) {
            return p.clone();
        }
        let found = lib
            .systems
            .iter()
            .find(|s| s.games.contains(&game))
            .and_then(|sys| self.finder.find(sys, g))
            .map(|(p, _)| p);
        self.paths.insert(g.key.clone(), found.clone());
        found
    }

    /// The cover for a game sized to fit `size` cells, kicking off a load if it isn't ready.
    pub fn get(&mut self, lib: &Library, game: usize, size: Size) -> Cover<'_> {
        if size.width < 4 || size.height < 3 {
            return Cover::None;
        }
        let Some(path) = self.path(lib, game) else { return Cover::None };
        let slot = (lib.games[game].key.clone(), size);
        if self.failed.contains(&slot) {
            return Cover::None;
        }
        if !self.ready.contains_key(&slot) {
            if self.requested.as_ref() != Some(&slot) {
                self.requested = Some(slot.clone());
                let _ = self.tx.send(Request { slot, path });
            }
            return Cover::Loading;
        }
        Cover::Ready(&self.ready[&slot])
    }

    /// Collects finished images. Returns true if anything new is ready to draw.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok((slot, proto)) = self.rx.try_recv() {
            if self.requested.as_ref() == Some(&slot) {
                self.requested = None;
            }
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
        changed
    }

    /// Forgets rendered images, e.g. after another program had the terminal: the terminal may
    /// have dropped image data we'd otherwise assume it still has.
    pub fn forget_rendered(&mut self) {
        self.ready.clear();
        self.order.clear();
        self.requested = None;
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
    picker.new_protocol(img, req.slot.1, Resize::Fit(Some(FilterType::Triangle))).ok()
}
