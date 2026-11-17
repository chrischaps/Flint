//! The window's replay (#1089): a worker thread that loads the captured
//! frame's scene once (`render::replay_host`) and draws it again each time
//! the controls change, writing each replay as a capture in a folder of its
//! own, which the window then shows in place of the captured one.
//!
//! The window never waits on it (a scene's load is a second or more): it
//! asks with [`Replay::ask`], polls with [`Replay::poll`], and while a
//! replay is drawing keeps the last one on screen. Controls changed while
//! one draws are drawn next, the latest only.

use super::super::read::Capture;
use super::super::replay::{matches, stepped_match, Match};
use crate::commands::render::replay_host::{AssetCheck, Controls, ReplayHost, Skippable};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// What the load found, for the panel.
#[derive(Debug, Clone, Default)]
pub struct Loaded {
    pub notes: Vec<String>,
    pub assets: Vec<AssetCheck>,
    pub fresh_only: Vec<String>,
    pub skippable: Vec<Skippable>,
}

/// One replay drawn.
#[derive(Debug, Clone)]
pub struct Drawn {
    pub dir: PathBuf,
    pub controls: Controls,
    /// Its final image's and scene pass's comparison with the captured
    /// ones, then the stepped pass's when it isn't the scene's (#1204).
    pub matches: Vec<Match>,
    pub refused: Vec<String>,
}

enum Msg {
    Loaded(Result<Loaded, String>),
    Drawn(Result<Drawn, String>),
}

/// Where the replay stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Not asked for.
    Off,
    /// The scene loading.
    Loading,
    Ready,
    Failed(String),
}

/// The window's replay. See the module docs.
pub struct Replay {
    source: PathBuf,
    scene: Option<PathBuf>,
    schemas: Vec<String>,
    tx: Option<mpsc::Sender<Controls>>,
    rx: Option<mpsc::Receiver<Msg>>,
    pub status: Status,
    pub loaded: Option<Loaded>,
    /// What the window asks for.
    pub want: Controls,
    /// What is drawing now.
    in_flight: Option<Controls>,
    /// The last replay drawn.
    pub last: Option<Drawn>,
}

impl Replay {
    pub fn new(source: &Path, scene: Option<PathBuf>, schemas: Vec<String>) -> Self {
        Self {
            source: source.to_path_buf(),
            scene,
            schemas,
            tx: None,
            rx: None,
            status: Status::Off,
            loaded: None,
            want: Controls::default(),
            in_flight: None,
            last: None,
        }
    }

    /// Load the scene on a worker thread and draw the frame whole.
    pub fn start(&mut self) {
        if self.status != Status::Off {
            return;
        }
        let (ask, asked) = mpsc::channel::<Controls>();
        let (tell, told) = mpsc::channel::<Msg>();
        let (source, scene, schemas) = (
            self.source.clone(),
            self.scene.clone(),
            self.schemas.clone(),
        );
        std::thread::Builder::new()
            .name("frame replay".into())
            // The scene load and the renderer's frame are deep in debug
            .stack_size(16 << 20)
            .spawn(move || worker(source, scene, schemas, asked, tell))
            .expect("the replay thread starts");
        self.tx = Some(ask);
        self.rx = Some(told);
        self.status = Status::Loading;
        self.send();
    }

    /// Whether a load or a draw is under way, or controls wait to be drawn.
    pub fn busy(&self) -> bool {
        self.status == Status::Loading
            || self.in_flight.is_some()
            || (self.status == Status::Ready
                && self.last.as_ref().is_none_or(|d| d.controls != self.want))
    }

    /// Ask for the frame under `controls` (drawn once the one in flight is).
    pub fn ask(&mut self, controls: Controls) {
        self.want = controls;
        self.send();
    }

    fn send(&mut self) {
        if self.in_flight.is_some() {
            return;
        }
        if self.last.as_ref().is_some_and(|d| d.controls == self.want) {
            return;
        }
        if let Some(tx) = &self.tx {
            if tx.send(self.want.clone()).is_ok() {
                self.in_flight = Some(self.want.clone());
            }
        }
    }

    /// Take what the worker said. Returns a replay newly drawn.
    pub fn poll(&mut self) -> Option<Drawn> {
        let mut newest = None;
        while let Some(msg) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            if let Some(d) = self.take(msg) {
                newest = Some(d);
            }
        }
        newest
    }

    /// Wait for whatever is under way: for a screenshot.
    pub fn wait(&mut self) -> Option<Drawn> {
        let mut newest = None;
        while self.busy() && self.status != Status::Off {
            let Some(msg) = self.rx.as_ref().and_then(|rx| rx.recv().ok()) else {
                self.status = Status::Failed("the replay thread stopped".into());
                break;
            };
            if let Some(d) = self.take(msg) {
                newest = Some(d);
            }
            if matches!(self.status, Status::Failed(_)) {
                break;
            }
        }
        newest
    }

    fn take(&mut self, msg: Msg) -> Option<Drawn> {
        match msg {
            Msg::Loaded(Ok(loaded)) => {
                self.loaded = Some(loaded);
                self.status = Status::Ready;
                None
            }
            Msg::Loaded(Err(e)) => {
                self.status = Status::Failed(e);
                self.in_flight = None;
                None
            }
            Msg::Drawn(result) => {
                self.in_flight = None;
                let drawn = match result {
                    Ok(d) => {
                        self.last = Some(d.clone());
                        Some(d)
                    }
                    Err(e) => {
                        self.status = Status::Failed(e);
                        None
                    }
                };
                self.send();
                drawn
            }
        }
    }
}

/// The worker: load once, then draw each controls asked for into a folder
/// of its own under the system's temporary folder, keeping the last few.
fn worker(
    source: PathBuf,
    scene: Option<PathBuf>,
    schemas: Vec<String>,
    asked: mpsc::Receiver<Controls>,
    tell: mpsc::Sender<Msg>,
) {
    let captured = match Capture::open(&source) {
        Ok(c) => c,
        Err(e) => {
            let _ = tell.send(Msg::Loaded(Err(format!("{e:#}"))));
            return;
        }
    };
    let mut host = match ReplayHost::load(
        &captured.dir,
        &captured.description,
        scene.as_deref(),
        &schemas,
    ) {
        Ok(h) => h,
        Err(e) => {
            let _ = tell.send(Msg::Loaded(Err(format!("{e:#}"))));
            return;
        }
    };
    let _ = tell.send(Msg::Loaded(Ok(Loaded {
        notes: host.notes.clone(),
        assets: host.assets.clone(),
        fresh_only: host.fresh_only.clone(),
        skippable: host.skippable.clone(),
    })));
    let root = std::env::temp_dir().join(format!("flint-replay-{}", std::process::id()));
    let mut n = 0usize;
    while let Ok(mut controls) = asked.recv() {
        // Only the latest of what waits
        while let Ok(later) = asked.try_recv() {
            controls = later;
        }
        let out = root.join(format!("r{n}.flintframe"));
        // Two back is no longer shown: the window has the newest open
        if n >= 2 {
            let _ = std::fs::remove_dir_all(root.join(format!("r{}.flintframe", n - 2)));
        }
        n += 1;
        let result = host
            .draw(&controls, &out)
            .and_then(|drawn| {
                let replayed = Capture::open(&out)?;
                let mut compared = matches(&captured, &replayed).to_vec();
                if controls.draws.is_some() {
                    compared.extend(stepped_match(&captured, &replayed, controls.stepped_name()));
                }
                Ok(Drawn {
                    dir: out.clone(),
                    controls: controls.clone(),
                    matches: compared,
                    refused: drawn.refused,
                })
            })
            .map_err(|e| format!("{e:#}"));
        if tell.send(Msg::Drawn(result)).is_err() {
            break;
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}
