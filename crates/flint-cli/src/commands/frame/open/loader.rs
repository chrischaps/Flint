//! Reading a capture's images off the UI thread: an EXR of a 1080p HDR
//! target takes a debug build long enough to drop frames, so each image is
//! decoded on a worker, made [`Prepared`] (four float channels, the
//! viewer's upload) and kept, the least recently asked for dropped first.

use super::super::display::{DepthCamera, Display, Prepared};
use super::super::read;
use flint_render::frame_capture::ImageDescription;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// An image read and ready to show.
pub struct Loaded {
    pub prepared: Prepared,
    /// How `flint frame image` would show it, and its words for that.
    pub auto: Display,
    pub words: String,
}

/// Where an image is.
#[derive(Clone)]
pub enum Slot {
    Pending,
    Ready(Arc<Loaded>),
    Failed(String),
}

struct Job {
    key: usize,
    path: PathBuf,
    image: ImageDescription,
    exposure: f32,
    /// The camera a scene depth was drawn from (#1176).
    camera: Option<DepthCamera>,
}

/// The images asked for, by their index in the model.
pub struct Loader {
    jobs: Sender<Job>,
    done: Receiver<(usize, Slot)>,
    slots: HashMap<usize, Slot>,
    /// Ready keys, least recently asked for first.
    order: VecDeque<usize>,
    /// How many ready images to keep.
    keep: usize,
}

impl Loader {
    /// A loader with `workers` threads, keeping `keep` images.
    pub fn new(workers: usize, keep: usize) -> Self {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel();
        let rx = Arc::new(Mutex::new(rx));
        for n in 0..workers.max(1) {
            let rx = Arc::clone(&rx);
            let tx = tx.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("frame-open-reader-{n}"))
                .spawn(move || loop {
                    let job = match rx.lock() {
                        Ok(rx) => match rx.recv() {
                            Ok(job) => job,
                            Err(_) => return,
                        },
                        Err(_) => return,
                    };
                    let slot = load(&job);
                    if tx.send((job.key, slot)).is_err() {
                        return;
                    }
                });
            if let Err(e) = spawned {
                tracing::warn!("couldn't start an image reader: {e}");
            }
        }
        Self {
            jobs,
            done,
            slots: HashMap::new(),
            order: VecDeque::new(),
            keep: keep.max(2),
        }
    }

    /// Ask for an image; it is read once, then kept.
    pub fn request(
        &mut self,
        key: usize,
        path: PathBuf,
        image: &ImageDescription,
        exposure: f32,
        camera: Option<DepthCamera>,
    ) {
        match self.slots.get(&key) {
            Some(Slot::Ready(_)) => {
                self.touch(key);
            }
            Some(_) => {}
            None => {
                self.slots.insert(key, Slot::Pending);
                let job = Job {
                    key,
                    path,
                    image: image.clone(),
                    exposure,
                    camera,
                };
                if self.jobs.send(job).is_err() {
                    self.slots
                        .insert(key, Slot::Failed("the image readers stopped".into()));
                }
            }
        }
    }

    pub fn get(&self, key: usize) -> Option<&Slot> {
        self.slots.get(&key)
    }

    /// The ready image, if it is.
    pub fn ready(&self, key: usize) -> Option<Arc<Loaded>> {
        match self.slots.get(&key) {
            Some(Slot::Ready(l)) => Some(Arc::clone(l)),
            _ => None,
        }
    }

    /// Take what the workers finished; true when anything did.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok((key, slot)) = self.done.try_recv() {
            self.finish(key, slot);
            any = true;
        }
        any
    }

    /// Whether an image is still being read.
    pub fn pending(&self) -> bool {
        self.slots.values().any(|s| matches!(s, Slot::Pending))
    }

    /// Wait until every image asked for is read (a screenshot's frame).
    pub fn wait(&mut self) {
        while self.pending() {
            match self.done.recv() {
                Ok((key, slot)) => self.finish(key, slot),
                Err(_) => break,
            }
        }
    }

    fn finish(&mut self, key: usize, slot: Slot) {
        if matches!(slot, Slot::Ready(_)) {
            self.touch(key);
        }
        self.slots.insert(key, slot);
        while self.order.len() > self.keep {
            if let Some(old) = self.order.pop_front() {
                self.slots.remove(&old);
            }
        }
    }

    fn touch(&mut self, key: usize) {
        self.order.retain(|k| *k != key);
        self.order.push_back(key);
    }
}

fn load(job: &Job) -> Slot {
    match read::texels(&job.path, &job.image) {
        Ok(t) => {
            let prepared = Prepared::new(&t, &job.image);
            let (auto, words) = Display::auto(&prepared, job.exposure, job.camera);
            Slot::Ready(Arc::new(Loaded {
                prepared,
                auto,
                words,
            }))
        }
        Err(e) => Slot::Failed(format!("{e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_fails_and_says_so() {
        let mut l = Loader::new(1, 4);
        let mut image = ImageDescription::default();
        image.stored = "png".into();
        l.request(0, PathBuf::from("no/such/image.png"), &image, 1.0, None);
        assert!(l.pending());
        l.wait();
        assert!(!l.pending());
        assert!(matches!(l.get(0), Some(Slot::Failed(_))));
    }

    #[test]
    fn the_least_recently_asked_for_image_goes_first() {
        let dir = std::env::temp_dir().join(format!("frame-open-loader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("one.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        let mut image = ImageDescription::default();
        image.stored = "png".into();
        image.format = "Rgba8Unorm".into();
        let mut l = Loader::new(2, 2);
        for key in 0..3 {
            l.request(key, path.clone(), &image, 1.0, None);
            l.wait();
        }
        assert!(l.get(0).is_none(), "dropped");
        assert!(l.ready(2).is_some());
        let loaded = l.ready(1).unwrap();
        assert_eq!(
            loaded.prepared.stored(1, 1),
            Some(vec![10.0, 20.0, 30.0, 255.0])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
