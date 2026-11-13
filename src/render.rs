use crate::{document, model::Paths, storage::Library};
use crossbeam_channel::{bounded, Receiver, Sender};
use eframe::egui::{self, ColorImage, TextureHandle};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
};

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct PageKey {
    pub hash: String,
    pub page: u32,
    pub pixels: u32,
}

struct Request {
    key: PageKey,
    generation: u64,
    cancel: Arc<AtomicBool>,
}

struct Rendered {
    key: PageKey,
    generation: u64,
    result: Result<ColorImage, String>,
}

pub struct PageRenderer {
    tx: Option<Sender<Request>>,
    rx: Receiver<Rendered>,
    worker: Option<JoinHandle<()>>,
    generation: u64,
    cancel: Arc<AtomicBool>,
    pending: HashSet<PageKey>,
    textures: HashMap<PageKey, TextureHandle>,
    failures: HashMap<PageKey, String>,
    order: VecDeque<PageKey>,
    bytes: usize,
}

pub const CACHE_BYTES: usize = 128 * 1024 * 1024;
pub const CACHE_PAGES: usize = 12;

impl PageRenderer {
    pub fn new(paths: Paths, ctx: egui::Context) -> Self {
        let (tx, jobs) = bounded::<Request>(8);
        let (results, rx) = bounded(4);
        let worker = std::thread::spawn(move || {
            while let Ok(request) = jobs.recv() {
                if request.cancel.load(Ordering::Relaxed) {
                    continue;
                }
                let key = request.key;
                let result = Library::open(&paths)
                    .and_then(|db| db.pdf_path(&paths, &key.hash))
                    .and_then(|path| document::render(&path, key.page, key.pixels, &request.cancel))
                    .map(|image| {
                        ColorImage::from_rgba_unmultiplied(
                            [image.width() as usize, image.height() as usize],
                            image.as_raw(),
                        )
                    })
                    .map_err(|e| format!("{e:#}"));
                if request.cancel.load(Ordering::Relaxed) {
                    continue;
                }
                if results
                    .send(Rendered {
                        key,
                        generation: request.generation,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self {
            tx: Some(tx),
            rx,
            worker: Some(worker),
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            pending: HashSet::new(),
            textures: HashMap::new(),
            failures: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
        }
    }

    pub fn invalidate_requests(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        self.pending.clear();
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(done) = self.rx.try_recv() {
            if done.generation != self.generation {
                continue;
            }
            self.pending.remove(&done.key);
            match done.result {
                Ok(image) => {
                    let bytes = image.pixels.len() * 4;
                    while cache_over_limit(self.textures.len() + 1, self.bytes + bytes) {
                        let Some(old) = self.order.pop_front() else {
                            break;
                        };
                        if let Some(texture) = self.textures.remove(&old) {
                            self.bytes = self
                                .bytes
                                .saturating_sub(texture.size()[0] * texture.size()[1] * 4);
                        }
                    }
                    let texture = ctx.load_texture(
                        format!("{}-{}-{}", done.key.hash, done.key.page, done.key.pixels),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    if let Some(old) = self.textures.insert(done.key.clone(), texture) {
                        self.bytes = self.bytes.saturating_sub(old.size()[0] * old.size()[1] * 4);
                        self.order.retain(|key| key != &done.key);
                    }
                    self.bytes += bytes;
                    self.order.push_back(done.key);
                }
                Err(error) => {
                    if self.failures.len() >= 32 {
                        self.failures.clear();
                    }
                    self.failures.insert(done.key, error);
                }
            }
        }
    }

    pub fn request(&mut self, key: PageKey) {
        if self.textures.contains_key(&key)
            || self.pending.contains(&key)
            || self.failures.contains_key(&key)
        {
            return;
        }
        if self.tx.as_ref().is_some_and(|tx| {
            tx.try_send(Request {
                key: key.clone(),
                generation: self.generation,
                cancel: self.cancel.clone(),
            })
            .is_ok()
        }) {
            self.pending.insert(key);
        }
    }

    pub fn texture(&mut self, key: &PageKey) -> Option<TextureHandle> {
        if let Some(texture) = self.textures.get(key) {
            self.order.retain(|old| old != key);
            self.order.push_back(key.clone());
            Some(texture.clone())
        } else {
            None
        }
    }

    pub fn failure(&self, key: &PageKey) -> Option<&str> {
        self.failures.get(key).map(String::as_str)
    }

    pub fn retry(&mut self) {
        self.failures.clear();
    }

    pub fn cache_usage(&self) -> (usize, usize) {
        (self.textures.len(), self.bytes)
    }
}

impl Drop for PageRenderer {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.tx.take();
        // Disconnect results before joining so a full result queue cannot deadlock shutdown.
        let (_, empty) = bounded(0);
        let old = std::mem::replace(&mut self.rx, empty);
        drop(old);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn cache_over_limit(pages: usize, bytes: usize) -> bool {
    pages > CACHE_PAGES || bytes > CACHE_BYTES
}
