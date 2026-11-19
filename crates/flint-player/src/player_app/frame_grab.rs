//! The presented frame, as a picture (#583): what the window showed, scene
//! and HUD, read back without the frame ever waiting for it.
//!
//! Two things ask for one:
//!
//! - **A client of `--expression-listen`** sends a line with a `capture`
//!   key, `{"capture": "look.png", "id": 7}` (or `"capture": true` for a
//!   name from the scene and the game time). The next frame presented is
//!   written to `--capture-dir` (default `captures/` at the project's root)
//!   and the client hears one line back on the same connection:
//!   `{"captured": "<absolute path>", "id": 7, "width": .., "height": ..,
//!   "t": <game time>, "frame": <frame>, "state": <the last expression state
//!   the scripts heard>}`, or `{"capture_error": "<why>", "id": 7}`. A
//!   capture line is not a state: the scripts never hear it.
//! - **`--record DIR --record-fps N`** writes every presented frame that
//!   falls due at N a second as `DIR/frame_000001.png`, ..., with
//!   `frames.jsonl` (a line a frame: its file, game time, wall time and Unix
//!   time, for lining a recording up with a transcript) and, at exit,
//!   `frames.ffconcat`, which ffmpeg plays at the times the frames were
//!   shown (`ffmpeg -f concat -i frames.ffconcat -pix_fmt yuv420p out.mp4`).
//!   The rate is on the wall clock, or on game time under `--frame-step`.
//!
//! Neither touches the frame clock the scripts see. The frame copies the
//! surface into a staging buffer in its own encoder (one copy, after the
//! HUD), asks for the buffer's map after the one submission, and polls it on
//! later frames without waiting; a writer thread reads the pixels out,
//! encodes the PNG and writes it. At most [`STAGING_BUFFERS`] are out at
//! once: past that a recording skips a frame (counted, said at the end)
//! rather than hold one up, and a capture line waits for the next frame.
//! Native only: the web player writes no files and has no socket.
//!
//! The `.flintframe` frame capture (#1086, `frame_capture.rs`) is the
//! debugger's: every pass's image and what a replay needs. This is the
//! viewer's: the one image the person saw.

use serde_json::{json, Map, Value};
use std::collections::VecDeque;
use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The most staging buffers out at once (copied, mapping, or with the
/// writer). Each is a whole frame: 3.7 MB at 1280x720, 33 MB at 4K.
pub const STAGING_BUFFERS: usize = 4;
/// The fastest a recording takes frames, a second.
pub const MAX_RECORD_FPS: f64 = 240.0;
/// How long the writer waits on a client that isn't reading its replies.
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);
/// The longest name a capture line may give.
const MAX_NAME: usize = 128;

/// The far end of a connection, for replies. One a connection, shared by
/// every capture it asks for, so two replies never interleave.
#[derive(Clone)]
pub struct Reply(Arc<Mutex<TcpStream>>);

impl Reply {
    /// The writing half of `stream`, or `None` if it can't be cloned.
    pub fn of(stream: &TcpStream) -> Option<Self> {
        let clone = stream.try_clone().ok()?;
        let _ = clone.set_write_timeout(Some(REPLY_TIMEOUT));
        Some(Self(Arc::new(Mutex::new(clone))))
    }

    /// Send `value` as one line. A client gone or not reading is logged.
    pub fn send(&self, value: &Value) {
        let mut line = value.to_string();
        line.push('\n');
        let mut stream = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = stream.write_all(line.as_bytes()) {
            tracing::warn!(target: "expression", "capture reply not sent: {e}");
        }
    }
}

/// What a capture line names its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskName {
    /// `"capture": true`: `<scene stem>-t<game time>.png`, `-2` and on when
    /// that's taken
    Auto,
    /// A file name in the capture folder, `.png` added when it has none
    Named(String),
}

/// One capture line, waiting for the next frame.
pub struct CaptureAsk {
    /// The line's `id`, echoed in the reply
    pub id: Option<Value>,
    pub name: AskName,
    pub reply: Option<Reply>,
}

/// Whether a socket line is a capture line rather than a state: it has a
/// `capture` key.
pub fn is_capture_line(line: &Map<String, Value>) -> bool {
    line.contains_key("capture")
}

/// A capture line's id and name, or why it's refused. It takes `capture`
/// and `id` and nothing else, so a state that happened to carry a
/// `capture` field is refused aloud rather than swallowed.
pub fn parse_ask(line: &Map<String, Value>) -> Result<(Option<Value>, AskName), String> {
    let id = line.get("id").cloned();
    if let Some(other) = line.keys().find(|k| *k != "capture" && *k != "id") {
        return Err(format!(
            "a capture line takes `capture` and `id` only, not `{other}`; send the state on a line of its own"
        ));
    }
    let name = match &line["capture"] {
        Value::Bool(true) => AskName::Auto,
        Value::String(s) => AskName::Named(file_name(s)?),
        other => {
            return Err(format!(
                "`capture` is a file name or true, not {}",
                serde_json::to_string(other).unwrap_or_default()
            ))
        }
    };
    Ok((id, name))
}

/// `name` as a PNG's file name in the capture folder: a plain name, no
/// folders, `.png` added when it has no extension. The folder is the
/// player's, so a client on the machine can't write anywhere else.
pub fn file_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("an empty file name".into());
    }
    if name.len() > MAX_NAME {
        return Err(format!("a file name of at most {MAX_NAME} bytes"));
    }
    if name.starts_with('.')
        || name.chars().any(|c| {
            matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
        })
    {
        return Err(format!(
            "`{name}`: a plain file name, written in the capture folder (no folders, no leading dot)"
        ));
    }
    match Path::new(name).extension().and_then(|e| e.to_str()) {
        None => Ok(format!("{name}.png")),
        Some(e) if e.eq_ignore_ascii_case("png") => Ok(name.to_string()),
        Some(e) => Err(format!("`{name}`: a capture is a PNG, not .{e}")),
    }
}

/// When a frame was shown, for its file and its line.
#[derive(Debug, Clone, Copy)]
pub struct GrabClock {
    /// Game time after the frame's tick
    pub game_time: f64,
    /// Whether the clock is stepped (`--frame-step`): the recording's rate
    /// is then on game time
    pub stepped: bool,
}

/// What the frame knows when it copies: where capture lines' PNGs go
/// (asked only while one waits), and the scene for an automatic name.
pub struct GrabPlace<'a> {
    pub capture_dir: Option<&'a Path>,
    pub scene_path: &'a str,
}

/// `--record`'s state on the frame thread.
struct Recording {
    dir: PathBuf,
    fps: f64,
    /// When the next frame falls due, on the recording's clock; `None`
    /// before the first, which is always due
    next_due: Option<f64>,
    /// The wall clock's zero: the first frame looked at
    start: Option<web_time::Instant>,
    taken: u64,
    skipped: u64,
    said_skip: bool,
}

impl Recording {
    /// The recording's clock now: game time when stepped, else wall seconds
    /// since its first frame.
    fn clock(&mut self, at: GrabClock, now: web_time::Instant) -> f64 {
        if at.stepped {
            at.game_time
        } else {
            now.duration_since(*self.start.get_or_insert(now))
                .as_secs_f64()
        }
    }

    /// Whether this frame falls due, and moves the due time on if it does.
    /// The due times are a grid from the first frame, so 8 a second under
    /// `--frame-step 1/60` lands on the same frames every run; a recording
    /// that fell behind (a stall) starts the grid again rather than
    /// bunching frames up to catch up.
    fn due(&mut self, clock: f64) -> bool {
        let interval = 1.0 / self.fps;
        let next = match self.next_due {
            None => clock + interval,
            Some(due) if clock + 1e-6 < due => return false,
            Some(due) if due + interval + 1e-6 < clock => clock + interval,
            Some(due) => due + interval,
        };
        self.next_due = Some(next);
        true
    }
}

/// One output of a frame's picture.
enum Output {
    Ask {
        path: PathBuf,
        id: Option<Value>,
        reply: Option<Reply>,
        state: Option<Map<String, Value>>,
    },
    Record {
        n: u64,
        /// The recording's clock at the frame (see [`Recording::clock`])
        clock: f64,
    },
}

/// A staging buffer the size of the surface.
struct Staging {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_row: u32,
}

/// A frame's copy on its way back.
struct InFlight {
    staging: Staging,
    format: wgpu::TextureFormat,
    /// 0 waiting, 1 mapped, 2 failed; set by the map's callback
    mapped: Arc<AtomicU8>,
    /// Whether the map has been asked for (after the submission)
    asked: bool,
    facts: Facts,
    outputs: Vec<Output>,
}

#[derive(Debug, Clone, Copy)]
struct Facts {
    frame: u64,
    game_time: f64,
    /// Wall seconds since the run's first grab
    wall: f64,
    unix_ms: u128,
}

enum Job {
    Frame(Box<InFlight>),
    /// Asks the frame couldn't take, with why
    Refuse(Vec<CaptureAsk>, String),
}

/// What the writer did, when it's joined.
#[derive(Debug, Default)]
struct Written {
    recorded: u64,
    failure: Option<String>,
}

/// The player's frame grabs: the capture lines waiting, the recording, and
/// the copies on their way back.
#[derive(Default)]
pub struct FrameGrabs {
    asks: Vec<CaptureAsk>,
    /// The last expression state the scripts heard, for the replies
    state: Option<Map<String, Value>>,
    record: Option<Recording>,
    /// Buffers back from the writer, ready to copy into again
    free: Vec<Staging>,
    /// Buffers made (free, in flight or with the writer)
    made: usize,
    in_flight: VecDeque<InFlight>,
    writer: Option<Writer>,
    frames: u64,
    wall_start: Option<web_time::Instant>,
    /// Why the recording stopped, or the writer failed
    failure: Option<String>,
    finished: bool,
}

struct Writer {
    jobs: Sender<Job>,
    back: Receiver<Staging>,
    thread: std::thread::JoinHandle<Written>,
}

impl FrameGrabs {
    /// Record every frame due at `fps` into `dir` (`--record`), which
    /// [`check_record_dir`] has checked.
    pub fn record_into(&mut self, dir: PathBuf, fps: f64) {
        self.record = Some(Recording {
            dir,
            fps,
            next_due: None,
            start: None,
            taken: 0,
            skipped: 0,
            said_skip: false,
        });
    }

    /// Capture lines that reached the frame, and the state the scripts
    /// heard last.
    pub fn ask(&mut self, asks: Vec<CaptureAsk>, state: Option<Map<String, Value>>) {
        if !asks.is_empty() {
            self.state = state;
            self.asks.extend(asks);
        }
    }

    /// Whether a capture line waits for a frame.
    pub fn has_asks(&self) -> bool {
        !self.asks.is_empty()
    }

    /// Whether anything waits for a frame or on the GPU: with nothing, the
    /// frame skips every grab call.
    pub fn idle(&self) -> bool {
        self.asks.is_empty() && self.record.is_none() && self.in_flight.is_empty()
    }

    /// Before the frame's one submission, after the HUD: copy `texture`
    /// into a staging buffer when a capture line waits or the recording
    /// falls due. `copyable` is the surface's `COPY_SRC`.
    pub fn before_submit(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        copyable: bool,
        at: GrabClock,
        place: &GrabPlace<'_>,
    ) {
        if self.finished || (self.asks.is_empty() && self.record.is_none()) {
            return;
        }
        self.frames += 1;
        let now = web_time::Instant::now();
        let wall = now
            .duration_since(*self.wall_start.get_or_insert(now))
            .as_secs_f64();
        let record_due = self.record.as_mut().and_then(|r| {
            let clock = r.clock(at, now);
            r.due(clock).then_some(clock)
        });
        if self.asks.is_empty() && record_due.is_none() {
            return;
        }
        if !copyable {
            let why = "the window's surface can't be copied on this adapter (no COPY_SRC)";
            let asks = std::mem::take(&mut self.asks);
            if !asks.is_empty() {
                self.send(Job::Refuse(asks, why.into()));
            }
            if let Some(r) = self.record.take() {
                tracing::warn!(target: "player", "[record] stopped: {why}");
                self.failure = Some(format!("--record {}: {why}", r.dir.display()));
            }
            return;
        }
        let (width, height) = (texture.width(), texture.height());
        let Some(staging) = self.staging(device, width, height) else {
            // Every buffer is out: a capture line waits for the next frame,
            // the recording passes this one over
            if record_due.is_some() {
                if let Some(r) = &mut self.record {
                    r.skipped += 1;
                    if !r.said_skip {
                        r.said_skip = true;
                        tracing::warn!(
                            target: "player",
                            "[record] skipped a frame: {STAGING_BUFFERS} frames were still on their way \
                             to the disk (the writer is behind; a lower --record-fps keeps up)"
                        );
                    }
                }
            }
            return;
        };
        let mut outputs = Vec::new();
        let mut taken: Vec<PathBuf> = Vec::new();
        let capture_dir = place.capture_dir.unwrap_or(Path::new("captures"));
        for ask in std::mem::take(&mut self.asks) {
            let name = match &ask.name {
                AskName::Named(n) => n.clone(),
                AskName::Auto => auto_name(capture_dir, place.scene_path, at.game_time, &taken),
            };
            let path = capture_dir.join(name);
            taken.push(path.clone());
            outputs.push(Output::Ask {
                path,
                id: ask.id,
                reply: ask.reply,
                state: self.state.clone(),
            });
        }
        if let (Some(clock), Some(r)) = (record_due, &mut self.record) {
            r.taken += 1;
            outputs.push(Output::Record { n: r.taken, clock });
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(staging.padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.in_flight.push_back(InFlight {
            staging,
            format: texture.format(),
            mapped: Arc::new(AtomicU8::new(0)),
            asked: false,
            facts: Facts {
                frame: self.frames,
                game_time: at.game_time,
                wall,
                unix_ms: unix_ms(),
            },
            outputs,
        });
    }

    /// After the submission: ask for the new copies' maps.
    pub fn submitted(&mut self) {
        for grab in self.in_flight.iter_mut().filter(|g| !g.asked) {
            grab.asked = true;
            let mapped = grab.mapped.clone();
            grab.staging
                .buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    mapped.store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
                });
        }
    }

    /// Each frame: poll the device without waiting and hand every copy
    /// that's back to the writer, oldest first.
    pub fn poll(&mut self, device: &wgpu::Device) {
        if self.in_flight.is_empty() {
            return;
        }
        let _ = device.poll(wgpu::PollType::Poll);
        self.hand_over_ready();
    }

    fn hand_over_ready(&mut self) {
        while let Some(front) = self.in_flight.front() {
            match front.mapped.load(Ordering::Acquire) {
                0 => break,
                1 => {
                    let grab = self.in_flight.pop_front().expect("the front");
                    self.send(Job::Frame(Box::new(grab)));
                }
                _ => {
                    let grab = self.in_flight.pop_front().expect("the front");
                    self.made -= 1;
                    let asks = grab
                        .outputs
                        .into_iter()
                        .filter_map(|o| match o {
                            Output::Ask { id, reply, .. } => Some(CaptureAsk {
                                id,
                                name: AskName::Auto,
                                reply,
                            }),
                            Output::Record { .. } => None,
                        })
                        .collect::<Vec<_>>();
                    tracing::warn!(target: "player", "[capture] a frame's read-back failed");
                    if !asks.is_empty() {
                        self.send(Job::Refuse(asks, "the frame's read-back failed".into()));
                    }
                }
            }
        }
    }

    /// At the run's end: wait for the copies still out (the frames are
    /// over, so waiting costs nothing), let the writer finish, write
    /// `frames.ffconcat`, and say what was recorded.
    pub fn finish(&mut self, device: &wgpu::Device) {
        if self.finished {
            return;
        }
        self.finished = true;
        if !self.in_flight.is_empty() {
            self.submitted();
            let _ = flint_render::wait_for_gpu(device);
            self.hand_over_ready();
        }
        self.join_writer();
    }

    /// [`Self::finish`] with no device (the window never opened): the
    /// writer's queue only.
    pub fn finish_without_device(&mut self) {
        if !self.finished {
            self.finished = true;
            self.join_writer();
        }
    }

    fn join_writer(&mut self) {
        // Capture lines that never got a frame hear why
        let asks = std::mem::take(&mut self.asks);
        if !asks.is_empty() {
            self.send(Job::Refuse(
                asks,
                "the player closed before the next frame".into(),
            ));
        }
        let written = match self.writer.take() {
            Some(writer) => {
                drop(writer.jobs);
                writer.thread.join().unwrap_or_else(|_| Written {
                    recorded: 0,
                    failure: Some("the frame writer panicked".into()),
                })
            }
            None => Written::default(),
        };
        if let Some(f) = written.failure {
            self.failure.get_or_insert(f);
        }
        if let Some(r) = &self.record {
            let skipped = match r.skipped {
                0 => String::new(),
                n => format!("; {n} skipped while the writer was behind"),
            };
            tracing::info!(
                target: "report",
                frames = written.recorded,
                skipped = r.skipped,
                "[record] {} frames at {} fps to {} (frames.jsonl, frames.ffconcat){skipped}",
                written.recorded,
                r.fps,
                flint_core::project::shown(&r.dir)
            );
        }
    }

    /// Why the recording stopped or a frame wasn't written, for
    /// `run_failure`.
    pub fn failure(&self) -> Option<String> {
        self.failure.clone()
    }

    /// A free buffer for a `width` x `height` copy, made when fewer than
    /// [`STAGING_BUFFERS`] exist. A buffer of another size (the window was
    /// resized) is dropped.
    fn staging(&mut self, device: &wgpu::Device, width: u32, height: u32) -> Option<Staging> {
        if let Some(writer) = &self.writer {
            self.free.extend(writer.back.try_iter());
        }
        let before = self.free.len();
        self.free.retain(|s| s.width == width && s.height == height);
        self.made -= before - self.free.len();
        if let Some(s) = self.free.pop() {
            return Some(s);
        }
        if self.made >= STAGING_BUFFERS {
            return None;
        }
        self.made += 1;
        let padded_row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        Some(Staging {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Frame Grab Staging"),
                size: u64::from(padded_row) * u64::from(height),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            width,
            height,
            padded_row,
        })
    }

    fn send(&mut self, job: Job) {
        let record = self.record.as_ref().map(|r| (r.dir.clone(), r.fps));
        let writer = self.writer.get_or_insert_with(|| Writer::start(record));
        if writer.jobs.send(job).is_err() {
            self.failure
                .get_or_insert_with(|| "the frame writer stopped".into());
        }
    }
}

impl Writer {
    fn start(record: Option<(PathBuf, f64)>) -> Self {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (give_back, back) = mpsc::channel::<Staging>();
        let thread = std::thread::Builder::new()
            .name("frame-grab-writer".into())
            .spawn(move || write_all(inbox, give_back, record))
            .expect("the frame writer's thread starts");
        Self { jobs, back, thread }
    }
}

/// The writer thread: every job in order, then `frames.ffconcat`.
fn write_all(
    inbox: Receiver<Job>,
    give_back: Sender<Staging>,
    record: Option<(PathBuf, f64)>,
) -> Written {
    let mut written = Written::default();
    let mut log = record
        .as_ref()
        .map(|(dir, fps)| RecordLog::new(dir.clone(), *fps));
    for job in inbox {
        match job {
            Job::Refuse(asks, why) => {
                for ask in asks {
                    if let Some(reply) = &ask.reply {
                        reply.send(&error_line(ask.id.as_ref(), &why));
                    }
                }
            }
            Job::Frame(grab) => {
                let grab = *grab;
                let (w, h) = (grab.staging.width, grab.staging.height);
                let rgb = read_rgb(&grab.staging, grab.format);
                let _ = give_back.send(grab.staging);
                let png = encode_png(&rgb, w, h);
                for output in grab.outputs {
                    match output {
                        Output::Ask {
                            path,
                            id,
                            reply,
                            state,
                        } => {
                            let result = png
                                .as_ref()
                                .map_err(String::clone)
                                .and_then(|bytes| write_capture(&path, bytes));
                            match result {
                                Ok(path) => {
                                    tracing::info!(
                                        target: "player",
                                        "[capture] {} ({w}x{h} at t={:.3} s), asked on the socket",
                                        flint_core::project::shown(&path),
                                        grab.facts.game_time
                                    );
                                    if let Some(reply) = &reply {
                                        let mut line = json!({
                                            "captured": path.to_string_lossy(),
                                            "width": w,
                                            "height": h,
                                            "t": grab.facts.game_time,
                                            "frame": grab.facts.frame,
                                        });
                                        if let Some(id) = id {
                                            line["id"] = id;
                                        }
                                        if let Some(state) = state {
                                            line["state"] = Value::Object(state);
                                        }
                                        reply.send(&line);
                                    }
                                }
                                Err(why) => {
                                    tracing::warn!(target: "player", "[capture] {why}");
                                    if let Some(reply) = &reply {
                                        reply.send(&error_line(id.as_ref(), &why));
                                    }
                                }
                            }
                        }
                        Output::Record { n, clock } => {
                            let Some(log) = log.as_mut().filter(|l| l.failure.is_none()) else {
                                continue;
                            };
                            match &png {
                                Ok(bytes) => {
                                    if log.frame(n, clock, &grab.facts, bytes) {
                                        written.recorded += 1;
                                    }
                                }
                                Err(why) => log.fail(why.clone()),
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(log) = log {
        written.failure = log.finish();
    }
    written
}

/// The recording's files, on the writer thread.
struct RecordLog {
    dir: PathBuf,
    fps: f64,
    lines: Option<std::fs::File>,
    /// Each frame's file and its time on the recording's clock
    rows: Vec<(String, f64)>,
    failure: Option<String>,
}

impl RecordLog {
    fn new(dir: PathBuf, fps: f64) -> Self {
        #[allow(clippy::disallowed_methods)] // the recording's own file, written as it goes
        let lines = std::fs::File::create(dir.join("frames.jsonl"));
        let (lines, failure) = match lines {
            Ok(f) => (Some(f), None),
            Err(e) => (
                None,
                Some(format!(
                    "--record: can't write {}: {e}",
                    dir.join("frames.jsonl").display()
                )),
            ),
        };
        Self {
            dir,
            fps,
            lines,
            rows: Vec::new(),
            failure,
        }
    }

    fn fail(&mut self, why: String) {
        tracing::warn!(target: "player", "[record] stopped: {why}");
        self.failure.get_or_insert(why);
    }

    /// Write frame `n`; false (and the recording stopped) if it couldn't be.
    fn frame(&mut self, n: u64, clock: f64, facts: &Facts, png: &[u8]) -> bool {
        let file = format!("frame_{n:06}.png");
        let path = self.dir.join(&file);
        #[allow(clippy::disallowed_methods)] // a recorded frame, written by the writer thread
        if let Err(e) = std::fs::write(&path, png) {
            self.fail(format!("--record: can't write {}: {e}", path.display()));
            return false;
        }
        let line = json!({
            "n": n,
            "file": file,
            "game_time": facts.game_time,
            "wall_secs": facts.wall,
            "unix_ms": facts.unix_ms as u64,
            "frame": facts.frame,
        });
        if let Some(lines) = &mut self.lines {
            if let Err(e) = writeln!(lines, "{line}") {
                self.fail(format!("--record: can't write frames.jsonl: {e}"));
                return false;
            }
        }
        self.rows.push((file, clock));
        true
    }

    /// `frames.ffconcat`: each frame held until the next one's time, the
    /// last for one interval. The concat demuxer needs the last file named
    /// twice to honour its duration.
    fn finish(mut self) -> Option<String> {
        if let Some(lines) = &mut self.lines {
            let _ = lines.flush();
        }
        if self.rows.is_empty() {
            return self.failure;
        }
        let mut text = String::from("ffconcat version 1.0\n");
        for (i, (file, at)) in self.rows.iter().enumerate() {
            let next = self.rows.get(i + 1).map_or(at + 1.0 / self.fps, |r| r.1);
            text.push_str(&format!(
                "file '{file}'\nduration {:.6}\n",
                (next - at).max(1e-3)
            ));
        }
        if let Some((last, _)) = self.rows.last() {
            text.push_str(&format!("file '{last}'\n"));
        }
        let path = self.dir.join("frames.ffconcat");
        if let Err(e) = flint_runtime::atomic_file::write(&path, &text) {
            self.failure
                .get_or_insert(format!("--record: can't write {}: {e}", path.display()));
        }
        self.failure
    }
}

/// Check `--record`'s folder before a window opens: made if it's missing,
/// refused if it already holds a recording.
pub fn check_record_dir(dir: &Path, fps: f64) -> anyhow::Result<()> {
    if !fps.is_finite() || fps <= 0.0 || fps > MAX_RECORD_FPS {
        anyhow::bail!(
            "--record-fps {fps}: frames a second, more than 0 and at most {MAX_RECORD_FPS}"
        );
    }
    #[allow(clippy::disallowed_methods)] // the recording's folder, made before the run
    std::fs::create_dir_all(dir)
        .map_err(|e| anyhow::anyhow!("--record {}: can't make the folder: {e}", dir.display()))?;
    #[allow(clippy::disallowed_methods)] // a write path's check, not an asset read
    if dir.join("frames.jsonl").exists() {
        anyhow::bail!(
            "--record {}: already holds a recording (frames.jsonl); give another folder or delete it",
            dir.display()
        );
    }
    Ok(())
}

/// `<scene stem>-t<game time>.png` in `dir`, with `-2`, `-3`... when a file
/// of that name exists or this frame already named it.
fn auto_name(dir: &Path, scene_path: &str, game_time: f64, taken: &[PathBuf]) -> String {
    let stem = Path::new(scene_path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.split('.').next().unwrap_or(n))
        .filter(|s| !s.is_empty())
        .unwrap_or("frame");
    let base = format!("{stem}-t{game_time:.3}");
    let mut n = 1;
    loop {
        let name = if n == 1 {
            format!("{base}.png")
        } else {
            format!("{base}-{n}.png")
        };
        let path = dir.join(&name);
        #[allow(clippy::disallowed_methods)] // a write path's check, not an asset read
        let exists = path.exists();
        if !exists && !taken.contains(&path) {
            return name;
        }
        n += 1;
    }
}

/// The mapped copy as tightly packed RGB, alpha dropped (a swapchain's is
/// undefined), BGRA swizzled. Unmaps the buffer.
fn read_rgb(staging: &Staging, format: wgpu::TextureFormat) -> Vec<u8> {
    let (w, h) = (staging.width as usize, staging.height as usize);
    let bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let rgb = {
        let slice = staging.buffer.slice(..);
        let data = slice.get_mapped_range().expect("a mapped buffer's range");
        flint_pixels::rgb8_from_rows(&data, staging.padded_row as usize, w, h, bgra)
    };
    staging.buffer.unmap();
    rgb
}

/// A PNG of `rgb`, fast compression: the writer keeps up with a recording
/// before it saves bytes.
fn encode_png(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder;
    let mut out = Vec::with_capacity(rgb.len() / 3);
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
        .write_image(rgb, width, height, image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("the frame's PNG couldn't be encoded: {e}"))?;
    Ok(out)
}

/// Write a capture line's PNG whole (a temp file, then a rename), so a
/// client reading the last one of that name never sees half of the next.
/// Returns the absolute path.
fn write_capture(path: &Path, png: &[u8]) -> Result<PathBuf, String> {
    flint_runtime::atomic_file::write_with(path, |file| file.write_all(png))
        .map_err(|e| format!("can't write {}: {e}", path.display()))?;
    Ok(std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

fn error_line(id: Option<&Value>, why: &str) -> Value {
    let mut line = json!({ "capture_error": why });
    if let Some(id) = id {
        line["id"] = id.clone();
    }
    line
}

fn unix_ms() -> u128 {
    web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

impl super::PlayerApp {
    /// `--record DIR --record-fps FPS` (#583): every presented frame due at
    /// `fps` a second, as PNGs in `dir`, which [`check_record_dir`] has
    /// checked. Before `run_app`.
    pub fn record_frames(&mut self, dir: PathBuf, fps: f64) {
        self.frame_grabs.record_into(dir, fps);
    }

    /// The socket's capture lines, taken from the expression feed into the
    /// grabs, at the start of `render`.
    pub(super) fn take_capture_asks(&mut self) {
        let feed = self
            .core
            .subsystems
            .find::<super::expression_feed::ExpressionSubsystem>();
        if let Some((asks, state)) = feed.map(|f| f.take_capture_asks()) {
            self.frame_grabs.ask(asks, state);
        }
    }

    /// Where a capture line's PNG goes: `--capture-dir`, else `captures/`
    /// at the project's root, as a `.flintframe` capture's.
    pub(super) fn capture_line_dir(&self) -> PathBuf {
        self.frame_captures.dir.clone().unwrap_or_else(|| {
            let scene_dir = Path::new(&self.core.scene_path)
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf();
            self.core.project.root_for(&scene_dir).join("captures")
        })
    }

    /// The run is ending: the grabs still out are written and the writer
    /// joined.
    pub(super) fn finish_frame_grabs(&mut self) {
        let device = match &self.render_context {
            Some(c) => Some(&c.device),
            #[cfg(test)]
            None => self.test_gpu.map(|g| &g.device),
            #[cfg(not(test))]
            None => None,
        };
        match device {
            Some(device) => self.frame_grabs.finish(device),
            None => self.frame_grabs.finish_without_device(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // makes and reads back a recording folder
mod tests {
    use super::*;

    fn line(text: &str) -> Map<String, Value> {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn a_capture_line_names_a_png_in_the_folder() {
        assert!(is_capture_line(&line(r#"{"capture": "look.png"}"#)));
        assert!(!is_capture_line(&line(r#"{"state": "listening"}"#)));
        assert_eq!(
            parse_ask(&line(r#"{"capture": "look", "id": 7}"#)).unwrap(),
            (Some(json!(7)), AskName::Named("look.png".into()))
        );
        assert_eq!(
            parse_ask(&line(r#"{"capture": true}"#)).unwrap(),
            (None, AskName::Auto)
        );
        assert_eq!(file_name("Look.PNG").unwrap(), "Look.PNG");
        for bad in [
            "", "../x.png", "a/b.png", "a\\b.png", "C:x.png", ".hidden", "x.jpg",
        ] {
            assert!(file_name(bad).is_err(), "{bad:?} is refused");
        }
        for bad in [
            r#"{"capture": false}"#,
            r#"{"capture": 3}"#,
            r#"{"capture": "a.png", "state": "x"}"#,
        ] {
            let why = parse_ask(&line(bad)).unwrap_err();
            assert!(!why.is_empty(), "{bad}");
        }
        assert!(parse_ask(&line(r#"{"capture": "a", "state": "x"}"#))
            .unwrap_err()
            .contains("`state`"));
    }

    #[test]
    fn a_recording_falls_due_on_a_grid_from_its_first_frame() {
        let mut r = Recording {
            dir: PathBuf::new(),
            fps: 8.0,
            next_due: None,
            start: None,
            taken: 0,
            skipped: 0,
            said_skip: false,
        };
        // --frame-step 1/60: 8 a second is every 7.5 frames, on the grid
        let due: Vec<u32> = (0..=60).filter(|i| r.due(f64::from(*i) / 60.0)).collect();
        assert_eq!(due, [0, 8, 15, 23, 30, 38, 45, 53, 60]);
        // A stall of a second starts the grid again instead of bunching up
        assert!(r.due(2.0));
        assert!(!r.due(2.01));
        assert!(r.due(2.125));
    }

    #[test]
    fn an_automatic_name_steps_past_one_taken() {
        let dir = std::env::temp_dir().join(format!("flint-grab-names-{}", std::process::id()));
        let first = auto_name(&dir, "levels/tavern.scene.toml", 1.5, &[]);
        assert_eq!(first, "tavern-t1.500.png");
        let second = auto_name(&dir, "levels/tavern.scene.toml", 1.5, &[dir.join(&first)]);
        assert_eq!(second, "tavern-t1.500-2.png");
    }

    #[test]
    fn a_folder_with_a_recording_is_refused() {
        let dir = std::env::temp_dir().join(format!("flint-grab-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        check_record_dir(&dir, 8.0).expect("a new folder is made");
        assert!(dir.is_dir());
        std::fs::write(dir.join("frames.jsonl"), "").unwrap();
        let why = check_record_dir(&dir, 8.0).unwrap_err().to_string();
        assert!(why.contains("already holds a recording"), "{why}");
        for fps in [0.0, -1.0, f64::NAN, 1000.0] {
            assert!(check_record_dir(&dir, fps).is_err(), "{fps}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
