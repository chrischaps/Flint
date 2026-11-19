//! Expression input from outside the process (Flint#518).
//!
//! Scripts read the latest state with `expression()` and hear each new one in
//! `on_expression(state)`. Two sources feed them, either or both:
//!
//! - **A timeline** (`--expression-timeline <file>`), played on the game
//!   clock, so `--frame-step` and `set_time_scale` apply to it as they do to
//!   everything else:
//!
//!   ```json
//!   {"events": [
//!     {"state": "arriving", "energy": 0.3, "t": 0.0},
//!     {"state": "listening", "energy": 0.25, "t": 2.0}
//!   ]}
//!   ```
//!
//!   Each event is delivered once game time reaches its `t`, in file order.
//!   Other top-level fields (a `version`, say) are the format owner's
//!   business and are ignored here.
//!
//! - **A localhost socket** (`--expression-listen 127.0.0.1:<port>`), taking
//!   newline-delimited JSON objects over TCP, one state per line. It binds
//!   loopback only and is off unless the flag is given. It opens once the
//!   first frame has been presented (#584): until then a connection is
//!   refused, so a client that has connected knows a state it sends is heard
//!   on the frame it arrives at, not drained with others on a slow first
//!   frame. The address is checked when the flag is read, and the opening is
//!   one `listen` line in the log, which `--quiet` keeps. Reader threads parse
//!   each line and hand it to the frame through a bounded queue; a frame takes
//!   at most [`MAX_PER_FRAME`] and never waits on the network. A line that
//!   isn't a JSON object, or is longer than [`MAX_LINE_BYTES`], is logged and
//!   dropped. Under a flood the queue drops its oldest states, never the
//!   newest: each message is a whole snapshot, so the last one is the truth.
//!
//! Flint doesn't know the fields. Any JSON object becomes a Rhai map, with
//! every number a float so scripts can do arithmetic on it without caring
//! whether the sender wrote `1` or `1.0`. What the fields mean, and how strict
//! to be about them, belongs to whoever writes the messages.

use super::subsystem::{Lasts, Stage, StageCtx, Subsystem};
use rhai::{Dynamic, Map};
use serde_json::Value;

/// The longest line the socket takes, in bytes, newline excluded.
#[cfg(not(target_arch = "wasm32"))]
pub use super::loopback::MAX_LINE_BYTES;
/// The most socket messages a frame takes. The rest wait for the next one.
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_PER_FRAME: usize = 64;
/// The most socket messages held for the frame. Past this the oldest go, so a
/// flood never costs the latest state, which is the one that matters.
#[cfg(not(target_arch = "wasm32"))]
pub const QUEUE_CAPACITY: usize = 256;

/// A JSON value as a script sees it: objects are maps, numbers are floats.
pub fn json_to_dynamic(value: &Value) -> Dynamic {
    match value {
        Value::Null => Dynamic::UNIT,
        Value::Bool(b) => Dynamic::from_bool(*b),
        Value::Number(n) => Dynamic::from_float(n.as_f64().unwrap_or(0.0) as rhai::FLOAT),
        Value::String(s) => Dynamic::from(s.clone()),
        Value::Array(a) => Dynamic::from_array(a.iter().map(json_to_dynamic).collect()),
        Value::Object(o) => Dynamic::from_map(json_to_map(o)),
    }
}

/// A JSON object as the map `expression()` returns.
pub fn json_to_map(object: &serde_json::Map<String, Value>) -> Map {
    object
        .iter()
        .map(|(k, v)| (k.as_str().into(), json_to_dynamic(v)))
        .collect()
}

/// How far short of an event's `t` a clock can be and still have reached it: a
/// microsecond, far over what summing frame steps loses in an hour and far under
/// any frame.
pub const DUE_TOLERANCE: f64 = 1e-6;

/// One timeline event: when, and the whole state as written.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedState {
    pub t: f64,
    pub state: serde_json::Map<String, Value>,
}

/// An expression timeline, played forward on the game clock.
#[derive(Debug, Clone, Default)]
pub struct ExpressionTimeline {
    events: Vec<TimedState>,
    next: usize,
}

impl ExpressionTimeline {
    /// Read a timeline file through the vfs.
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let text = flint_core::vfs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("expression timeline {path}: {e}"))?;
        Self::parse(&text).map_err(|e| anyhow::anyhow!("expression timeline {path}: {e}"))
    }

    /// Parse `{"events": [{..., "t": secs}, ...]}`. Every event is an object
    /// with a finite `t` of 0 or more, and no event comes before the one
    /// above it. Events at the same `t` play in file order.
    pub fn parse(text: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let events = root
            .as_object()
            .ok_or("expected an object with an `events` array")?
            .get("events")
            .ok_or("no `events` array")?
            .as_array()
            .ok_or("`events` is not an array")?;
        let mut out = Vec::with_capacity(events.len());
        let mut last = 0.0;
        for (i, event) in events.iter().enumerate() {
            let state = event
                .as_object()
                .ok_or_else(|| format!("event {i}: not an object"))?;
            let t = state
                .get("t")
                .and_then(Value::as_f64)
                .ok_or_else(|| format!("event {i}: no numeric `t`"))?;
            if !t.is_finite() || t < 0.0 {
                return Err(format!("event {i}: `t` is {t}; it must be 0 or later"));
            }
            if t < last {
                return Err(format!(
                    "event {i}: `t` {t} comes before the event above it ({last})"
                ));
            }
            last = t;
            out.push(TimedState {
                t,
                state: state.clone(),
            });
        }
        Ok(Self {
            events: out,
            next: 0,
        })
    }

    /// Every event whose time has come by `now` and hasn't been played yet,
    /// in order. Several can come due in one long frame; none is skipped.
    ///
    /// "Come" is to within [`DUE_TOLERANCE`]: a clock that sums fixed steps
    /// lands a hair short of whole times (120 steps of 1/60 make 1.99999...), and
    /// a cue written for 2 s belongs to the frame at 2 s, not the one after (#8).
    pub fn due(&mut self, now: f64) -> &[TimedState] {
        let start = self.next;
        while self.next < self.events.len() && self.events[self.next].t <= now + DUE_TOLERANCE {
            self.next += 1;
        }
        &self.events[start..self.next]
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Whether every event has played.
    pub fn is_finished(&self) -> bool {
        self.next >= self.events.len()
    }

    /// How many events have played.
    pub fn played(&self) -> usize {
        self.next
    }

    /// When the next event plays, if any is left.
    pub fn next_time(&self) -> Option<f64> {
        self.events.get(self.next).map(|e| e.t)
    }

    /// Every event's time, in order.
    pub fn times(&self) -> Vec<f64> {
        self.events.iter().map(|e| e.t).collect()
    }
}

/// Where a state came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Timeline,
    Socket,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Timeline => "timeline",
            Source::Socket => "socket",
        }
    }
}

/// What the socket has seen this run (the F3 Expression panel, #525).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketCounts {
    pub addr: std::net::SocketAddr,
    /// Whether it is listening yet: it opens after the first frame (#584).
    pub open: bool,
    /// Connections open now.
    pub connections: usize,
    /// Lines that weren't a JSON object.
    pub malformed: usize,
    /// Lines longer than the socket takes.
    pub oversize: usize,
    /// States pushed out of the queue by newer ones, under a flood.
    pub flooded: usize,
}

/// One state as the feed delivered it: when (game time and the feed's own
/// frame count), from where, and the state as written.
#[derive(Debug, Clone, PartialEq)]
pub struct Arrival {
    pub at: f64,
    pub frame: u64,
    pub source: Source,
    pub state: serde_json::Map<String, Value>,
}

/// The feed's frame at whose start the socket opens: the second, once the
/// first frame (adapter, surface, shaders, the first draw) has been presented
/// (#584).
#[cfg(not(target_arch = "wasm32"))]
pub const SOCKET_OPENS_AT_FRAME: u64 = 2;

/// How many states the panel's strip shows, newest first.
pub const RECENT: usize = 12;

/// The last [`RECENT`] states the feed delivered, and how many in all.
#[derive(Debug, Clone, Default)]
pub struct FeedRecord {
    /// Oldest first; the newest is the last.
    recent: std::collections::VecDeque<Arrival>,
    delivered: u64,
}

#[cfg_attr(not(feature = "debug-hud"), allow(dead_code))] // the panel's reads
impl FeedRecord {
    pub fn push(&mut self, arrival: Arrival) {
        if self.recent.len() >= RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(arrival);
        self.delivered += 1;
    }

    /// The state `expression()` holds once scripts have heard it.
    pub fn latest(&self) -> Option<&Arrival> {
        self.recent.back()
    }

    /// Newest first.
    pub fn newest_first(&self) -> impl Iterator<Item = &Arrival> {
        self.recent.iter().rev()
    }

    pub fn delivered(&self) -> u64 {
        self.delivered
    }
}

/// Parse one socket line into a state, or say why it was dropped.
#[cfg(not(target_arch = "wasm32"))]
pub fn parse_line(line: &[u8]) -> Result<serde_json::Map<String, Value>, String> {
    match serde_json::from_slice::<Value>(line) {
        Ok(Value::Object(state)) => Ok(state),
        Ok(other) => Err(format!("not a JSON object: {}", kind(&other))),
        Err(e) => Err(format!("not JSON: {e}")),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use listener::ExpressionListener;

#[cfg(not(target_arch = "wasm32"))]
mod listener {
    use super::super::frame_grab::{self, CaptureAsk, Reply};
    use super::super::loopback::{self, read_capped_line, trim, Line};
    use super::*;
    use std::collections::VecDeque;
    use std::io::BufReader;
    use std::net::{SocketAddr, TcpStream};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    type State = serde_json::Map<String, Value>;

    /// What the reader threads and the frame share. The lock is held only to
    /// push or to take a frame's worth, never across a read.
    #[derive(Default)]
    struct Shared {
        queue: Mutex<VecDeque<State>>,
        /// States pushed out by newer ones since the frame last looked.
        dropped: AtomicUsize,
        /// What the run has dropped, by why (the F3 Expression panel, #525).
        malformed: AtomicUsize,
        oversize: AtomicUsize,
        flooded: AtomicUsize,
        /// Set when the listener is dropped, so readers stop.
        closed: Arc<AtomicBool>,
        /// Capture lines waiting for a frame (#583), which aren't states
        asks: Mutex<Vec<CaptureAsk>>,
    }

    impl Shared {
        fn push(&self, state: State) {
            let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            if q.len() >= QUEUE_CAPACITY {
                q.pop_front();
                self.dropped.fetch_add(1, Ordering::Relaxed);
                self.flooded.fetch_add(1, Ordering::Relaxed);
            }
            q.push_back(state);
        }
    }

    /// A loopback TCP listener taking one JSON state per line, read off the
    /// frame thread.
    pub struct ExpressionListener {
        shared: Arc<Shared>,
        addr: SocketAddr,
        /// Connections open now, kept by the reader threads.
        open: Arc<AtomicUsize>,
    }

    impl Drop for ExpressionListener {
        fn drop(&mut self) {
            self.shared.closed.store(true, Ordering::Relaxed);
        }
    }

    impl ExpressionListener {
        /// Bind `addr` (e.g. `127.0.0.1:7777`; port 0 picks one) and start
        /// accepting. Anything but a loopback address is refused: this is an
        /// input to the game from the same machine, never from a network.
        pub fn bind(addr: &str) -> anyhow::Result<Self> {
            let listener = loopback::bind("--expression-listen", addr)?;
            let addr = listener.local_addr()?;
            let shared = Arc::new(Shared::default());
            let for_readers = shared.clone();
            let open = loopback::accept_on_thread(
                listener,
                "expression-listen",
                shared.closed.clone(),
                move |stream, peer| read_lines(stream, &for_readers, peer),
            )?;
            tracing::info!(target: "expression", "listening on {addr}");
            Ok(Self { shared, addr, open })
        }

        /// Where it's listening (the real port when bound to port 0).
        pub fn local_addr(&self) -> SocketAddr {
            self.addr
        }

        /// Connections open now and what the run has dropped.
        pub fn counts(&self) -> SocketCounts {
            SocketCounts {
                addr: self.addr,
                open: true,
                connections: self.open.load(Ordering::SeqCst),
                malformed: self.shared.malformed.load(Ordering::Relaxed),
                oversize: self.shared.oversize.load(Ordering::Relaxed),
                flooded: self.shared.flooded.load(Ordering::Relaxed),
            }
        }

        /// The states that have arrived, at most `max`, oldest first. Waits on
        /// nothing but a lock a reader holds for one push.
        pub fn drain(&self, max: usize) -> Vec<State> {
            let taken: Vec<State> = {
                let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
                let n = q.len().min(max);
                q.drain(..n).collect()
            };
            let dropped = self.shared.dropped.swap(0, Ordering::Relaxed);
            if dropped > 0 {
                tracing::warn!(
                    target: "expression",
                    "{dropped} older states dropped: more than {QUEUE_CAPACITY} were waiting for a frame"
                );
            }
            taken
        }

        /// The capture lines that have arrived (#583), oldest first.
        pub fn take_asks(&self) -> Vec<CaptureAsk> {
            std::mem::take(&mut *self.shared.asks.lock().unwrap_or_else(|e| e.into_inner()))
        }
    }

    /// Read lines until the peer hangs up or the player goes away. A line
    /// with a `capture` key asks for the next frame (#583): it is queued
    /// with this connection's writing half, which its reply goes back on.
    fn read_lines(stream: TcpStream, shared: &Shared, peer: &str) {
        let reply = Reply::of(&stream);
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        while !shared.closed.load(Ordering::Relaxed) {
            line.clear();
            match read_capped_line(&mut reader, &mut line) {
                Ok(Line::Eof) => return,
                Ok(Line::TooLong) => {
                    shared.oversize.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(
                        target: "expression",
                        "dropped a line from {peer}: longer than {MAX_LINE_BYTES} bytes"
                    );
                    continue;
                }
                Ok(Line::Ready) => {}
                Err(e) => {
                    tracing::warn!(target: "expression", "{peer}: {e}");
                    return;
                }
            }
            let text = trim(&line);
            if text.is_empty() {
                continue;
            }
            match parse_line(text) {
                Ok(line) if frame_grab::is_capture_line(&line) => {
                    match frame_grab::parse_ask(&line) {
                        Ok((id, name)) => shared
                            .asks
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push(CaptureAsk {
                                id,
                                name,
                                reply: reply.clone(),
                            }),
                        Err(why) => {
                            tracing::warn!(target: "expression", "refused a capture line from {peer}: {why}");
                            if let Some(reply) = &reply {
                                let mut answer = serde_json::json!({ "capture_error": why });
                                if let Some(id) = line.get("id") {
                                    answer["id"] = id.clone();
                                }
                                reply.send(&answer);
                            }
                        }
                    }
                }
                Ok(state) => shared.push(state),
                Err(why) => {
                    shared.malformed.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(target: "expression", "dropped a line from {peer}: {why}");
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Write;
        use std::time::{Duration, Instant};

        fn wait_for(
            listener: &ExpressionListener,
            n: usize,
        ) -> Vec<serde_json::Map<String, Value>> {
            let mut got = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(5);
            while got.len() < n && Instant::now() < deadline {
                got.extend(listener.drain(MAX_PER_FRAME));
                std::thread::sleep(Duration::from_millis(2));
            }
            got
        }

        #[test]
        fn refuses_anything_but_loopback() {
            let err = ExpressionListener::bind("0.0.0.0:0")
                .err()
                .unwrap()
                .to_string();
            assert!(err.contains("loopback"), "{err}");
            assert!(
                ExpressionListener::bind("localhost:7777").is_err(),
                "a name isn't an IP"
            );
        }

        #[test]
        fn a_line_arrives_and_bad_ones_are_dropped() {
            let listener = ExpressionListener::bind("127.0.0.1:0").unwrap();
            let mut s = TcpStream::connect(listener.local_addr()).unwrap();
            let long = format!("{{\"pad\": \"{}\"}}\n", "x".repeat(MAX_LINE_BYTES + 10));
            s.write_all(b"not json\n").unwrap();
            s.write_all(b"[1, 2]\n").unwrap();
            s.write_all(long.as_bytes()).unwrap();
            s.write_all(b"\n   \n").unwrap();
            s.write_all(b"{\"state\": \"thinking\", \"energy\": 1}\r\n")
                .unwrap();
            s.write_all(b"{\"state\": \"speaking\"}").unwrap(); // no newline, then hang up
            drop(s);
            let got = wait_for(&listener, 2);
            assert_eq!(got.len(), 2, "{got:?}");
            assert_eq!(got[0]["state"], "thinking");
            assert_eq!(got[1]["state"], "speaking");
            // and nothing else trickles in
            std::thread::sleep(Duration::from_millis(50));
            assert!(listener.drain(MAX_PER_FRAME).is_empty());
            // the panel's counts (#525): two that weren't objects, one too long
            let counts = listener.counts();
            assert_eq!((counts.malformed, counts.oversize), (2, 1), "{counts:?}");
            assert_eq!(counts.flooded, 0);
        }

        #[test]
        fn drain_takes_at_most_max_and_keeps_the_rest_in_order() {
            let listener = ExpressionListener::bind("127.0.0.1:0").unwrap();
            let mut s = TcpStream::connect(listener.local_addr()).unwrap();
            for i in 0..10 {
                writeln!(s, "{{\"i\": {i}}}").unwrap();
            }
            drop(s);
            // Ten short lines over loopback are read in well under this.
            std::thread::sleep(Duration::from_millis(250));
            let first = listener.drain(4);
            assert_eq!(first.len(), 4);
            let rest = wait_for(&listener, 6);
            let order: Vec<f64> = first
                .iter()
                .chain(&rest)
                .map(|m| m["i"].as_f64().unwrap())
                .collect();
            assert_eq!(order, (0..10).map(f64::from).collect::<Vec<_>>());
        }

        #[test]
        fn a_flood_drops_the_oldest_and_keeps_the_last_word() {
            let listener = ExpressionListener::bind("127.0.0.1:0").unwrap();
            let mut s = TcpStream::connect(listener.local_addr()).unwrap();
            let mut flood = String::new();
            for i in 0..2000 {
                flood.push_str(&format!("{{\"i\": {i}}}\n"));
            }
            flood.push_str("{\"state\": \"settled\"}\n");
            s.write_all(flood.as_bytes()).unwrap();
            drop(s);
            std::thread::sleep(Duration::from_millis(300));
            let mut got = Vec::new();
            loop {
                let frame = listener.drain(MAX_PER_FRAME);
                if frame.is_empty() {
                    break;
                }
                assert!(frame.len() <= MAX_PER_FRAME);
                got.extend(frame);
            }
            assert_eq!(
                got.len(),
                QUEUE_CAPACITY,
                "the queue holds its capacity, no more"
            );
            assert_eq!(
                got.last().unwrap()["state"],
                "settled",
                "the last word survives"
            );
            assert_eq!(got[0]["i"], 2000 - (QUEUE_CAPACITY as i64 - 1));
            assert_eq!(listener.counts().flooded, 2001 - QUEUE_CAPACITY);
        }

        #[test]
        fn two_connections_both_reach_the_frame() {
            let listener = ExpressionListener::bind("127.0.0.1:0").unwrap();
            let mut a = TcpStream::connect(listener.local_addr()).unwrap();
            let mut b = TcpStream::connect(listener.local_addr()).unwrap();
            a.write_all(b"{\"from\": \"a\"}\n").unwrap();
            b.write_all(b"{\"from\": \"b\"}\n").unwrap();
            let got = wait_for(&listener, 2);
            let mut from: Vec<&str> = got.iter().map(|m| m["from"].as_str().unwrap()).collect();
            from.sort();
            assert_eq!(from, ["a", "b"]);
            assert_eq!(listener.counts().connections, 2, "both still open");
            drop((a, b));
            let deadline = Instant::now() + Duration::from_secs(5);
            while listener.counts().connections > 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            assert_eq!(listener.counts().connections, 0, "both closed");
        }
    }
}

/// Every expression source the player was started with.
#[derive(Default)]
pub struct ExpressionFeed {
    pub timeline: Option<ExpressionTimeline>,
    /// The socket, once [`ExpressionFeed::open_socket`] has bound it.
    #[cfg(not(target_arch = "wasm32"))]
    pub listener: Option<ExpressionListener>,
    /// Where the socket will listen, checked but not yet bound: it opens
    /// after the first frame (#584), so a connection that succeeds means
    /// frames are running.
    #[cfg(not(target_arch = "wasm32"))]
    pub listen_at: Option<std::net::SocketAddr>,
    /// Why the bind once frames ran failed, if it did; read it through
    /// [`ExpressionFeed::failure`].
    #[cfg(not(target_arch = "wasm32"))]
    pub socket_failure: Option<String>,
}

impl ExpressionFeed {
    /// The feed for `--expression-timeline` and `--expression-listen`, or
    /// `None` when neither was given.
    pub fn open(timeline: Option<&str>, listen: Option<&str>) -> anyhow::Result<Option<Self>> {
        if timeline.is_none() && listen.is_none() {
            return Ok(None);
        }
        let mut feed = Self::default();
        if let Some(path) = timeline {
            let t = ExpressionTimeline::load(path)?;
            tracing::info!(target: "player", "Expression timeline: {path} ({} events)", t.len());
            feed.timeline = Some(t);
        }
        if let Some(addr) = listen {
            #[cfg(not(target_arch = "wasm32"))]
            {
                // Checked now, so a malformed or non-loopback address fails
                // the run here; bound once frames run (#584). Never bound
                // and let go to test the port: a client that connects in
                // that moment has its handshake taken and then reset, losing
                // what it sends (the one reset in 21 launches looked so).
                let at = super::loopback::check("--expression-listen", addr)?;
                tracing::info!(target: "player",
                    "Expression socket {at} opens once the first frame is presented \
                     (one JSON object per line)"
                );
                feed.listen_at = Some(at);
            }
            #[cfg(target_arch = "wasm32")]
            anyhow::bail!("--expression-listen {addr}: no sockets on the web");
        }
        Ok(Some(feed))
    }

    /// The states due this frame at game time `now`, each with its source:
    /// the timeline's first, then whatever the socket has, at most
    /// [`MAX_PER_FRAME`]. Each is logged at info under the `expression`
    /// target.
    pub fn poll(&mut self, now: f64) -> Vec<(Source, serde_json::Map<String, Value>)> {
        let mut out = Vec::new();
        if let Some(timeline) = &mut self.timeline {
            for event in timeline.due(now) {
                log_state("timeline", now, &event.state);
                out.push((Source::Timeline, event.state.clone()));
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(listener) = &self.listener {
            for state in listener.drain(MAX_PER_FRAME) {
                log_state("socket", now, &state);
                out.push((Source::Socket, state));
            }
        }
        out
    }

    /// Bind the socket the flag asked for, now that frames are running
    /// (#584): from here a state that arrives is heard on the next frame.
    /// `frame` and `now` (game time) are for the one `listen` log line that
    /// says so, which a launcher can wait for. A bind that fails (the port
    /// is taken) is an error in the log and [`ExpressionFeed::failure`], and
    /// the subsystem ends the run, so a launcher waiting on the socket sees
    /// the player exit instead of waiting for ever. Returns where it
    /// listens; `None` when there was nothing to open or the bind failed.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_socket(&mut self, frame: u64, now: f64) -> Option<std::net::SocketAddr> {
        let at = self.listen_at.take()?;
        match ExpressionListener::bind(&at.to_string()) {
            Ok(l) => {
                let addr = l.local_addr();
                // The player's one log (#716), under `listen`, which --quiet keeps
                tracing::info!(
                    target: "listen",
                    addr = %addr,
                    frame,
                    t = now,
                    "[listen] expression: listening on {addr} from frame {frame} \
                     (game time {now:.3} s); a state sent now is heard on the frame it arrives at"
                );
                self.listener = Some(l);
                Some(addr)
            }
            Err(e) => {
                tracing::error!(target: "listen", "[listen] expression: {e}; ending the run");
                self.socket_failure = Some(e.to_string());
                None
            }
        }
    }

    /// Why the socket never opened, when it didn't: the run's failure.
    pub fn failure(&self) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        return self.socket_failure.clone();
        #[cfg(target_arch = "wasm32")]
        None
    }

    /// The socket's counts, when there is one: open, or waiting for the
    /// first frame.
    pub fn socket_counts(&self) -> Option<SocketCounts> {
        #[cfg(not(target_arch = "wasm32"))]
        return self
            .listener
            .as_ref()
            .map(ExpressionListener::counts)
            .or_else(|| {
                self.listen_at.map(|addr| SocketCounts {
                    addr,
                    open: false,
                    connections: 0,
                    malformed: 0,
                    oversize: 0,
                    flooded: 0,
                })
            });
        #[cfg(target_arch = "wasm32")]
        None
    }
}

fn log_state(source: &str, now: f64, state: &serde_json::Map<String, Value>) {
    if tracing::enabled!(target: "expression", tracing::Level::INFO) {
        let json = serde_json::to_string(state).unwrap_or_default();
        tracing::info!(target: "expression", "{now:8.3}s {source}: {json}");
    }
}

/// The feed as one of the player's subsystems (ADR 0117, #724): the first
/// and simplest. A run has it when its host gave a feed
/// (`LaunchOptions::expression_feed`), and keeps it across scenes, since the
/// timeline plays on the game clock and the socket stays open for the whole
/// run. At [`Stage::Input`] it queues the states due by now for the scripts'
/// update, which hears them before `on_update`.
///
/// It also keeps the last few states it delivered, which the F3 Expression
/// panel shows (#525, debug-hud): the panel is added at the first frame and
/// again after a scene's panels are dropped, and filled while it is open.
pub(crate) struct ExpressionSubsystem {
    feed: ExpressionFeed,
    record: FeedRecord,
    /// Frames this subsystem has seen at [`Stage::Input`].
    frames: u64,
}

impl ExpressionSubsystem {
    pub(crate) fn new(feed: ExpressionFeed) -> Self {
        Self {
            feed,
            record: FeedRecord::default(),
            frames: 0,
        }
    }

    /// Why the run failed, if the socket couldn't open (#584).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn failure(&self) -> Option<String> {
        self.feed.failure()
    }

    /// What the panel shows at game time `now`.
    #[cfg_attr(not(feature = "debug-hud"), allow(dead_code))]
    pub(crate) fn view(&self, now: f64) -> FeedView {
        FeedView {
            now,
            frame: self.frames,
            record: self.record.clone(),
            timeline: self.feed.timeline.as_ref().map(|t| TimelineView {
                played: t.played(),
                len: t.len(),
                next: t.next_time(),
            }),
            socket: self.feed.socket_counts(),
        }
    }

    /// Add the panel when the player has none (the first frame, or after a
    /// scene's panels went), and fill it while it's open.
    #[cfg(feature = "debug-hud")]
    fn feed_panel(&self, now: f64, panels: &mut Vec<Box<dyn flint_debug_ui::DebugPanel>>) {
        use super::expression_panel::{ExpressionPanel, EXPRESSION_PANEL};
        if !panels.iter().any(|p| p.as_any().is::<ExpressionPanel>()) {
            let times = self.feed.timeline.as_ref().map(|t| t.times());
            panels.push(Box::new(ExpressionPanel::new(times)));
            tracing::debug!(target: "expression", "{EXPRESSION_PANEL} panel added (F3)");
        }
        let panel = panels
            .iter_mut()
            .find_map(|p| p.as_any_mut().downcast_mut::<ExpressionPanel>());
        if let Some(panel) = panel.filter(|p| flint_debug_ui::DebugPanel::is_open(*p)) {
            panel.set_view(self.view(now));
        }
    }
}

/// The feed as the panel shows it: a copy, taken once a frame while open.
#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "debug-hud"), allow(dead_code))]
pub struct FeedView {
    /// Game time now.
    pub now: f64,
    /// The feed's frame count now.
    pub frame: u64,
    pub record: FeedRecord,
    pub timeline: Option<TimelineView>,
    pub socket: Option<SocketCounts>,
}

/// Where a timeline stands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimelineView {
    pub played: usize,
    pub len: usize,
    /// When the next event plays; `None` once all have.
    pub next: Option<f64>,
}

impl ExpressionSubsystem {
    /// The socket's capture lines (#583), with the last state the scripts
    /// heard, which each reply carries.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn take_capture_asks(
        &self,
    ) -> (
        Vec<super::frame_grab::CaptureAsk>,
        Option<serde_json::Map<String, Value>>,
    ) {
        let asks = self
            .feed
            .listener
            .as_ref()
            .map(ExpressionListener::take_asks)
            .unwrap_or_default();
        let state = if asks.is_empty() {
            None
        } else {
            self.record.latest().map(|a| a.state.clone())
        };
        (asks, state)
    }
}

impl Subsystem for ExpressionSubsystem {
    fn name(&self) -> &'static str {
        "expression feed"
    }

    fn lasts(&self) -> Lasts {
        Lasts::Run
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        if stage != Stage::Input {
            return;
        }
        self.frames += 1;
        let now = ctx.clock.total_time;
        // The second frame's start: the first has been presented, so frames
        // are running and the socket opens (#584)
        #[cfg(not(target_arch = "wasm32"))]
        if self.frames == SOCKET_OPENS_AT_FRAME {
            self.feed.open_socket(self.frames, now);
            if self.feed.failure().is_some() {
                // A port taken: the run ends, and says why (`run_failure`)
                if let Some(quit) = ctx.quit.as_deref_mut() {
                    *quit = true;
                }
            }
        }
        // The timeline on the game clock, then what the socket's reader
        // threads have parsed
        for (source, state) in self.feed.poll(now) {
            ctx.script.queue_expression(json_to_map(&state));
            self.record.push(Arrival {
                at: now,
                frame: self.frames,
                source,
                state,
            });
        }
        #[cfg(feature = "debug-hud")]
        self.feed_panel(now, ctx.panels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_TURN: &str = r#"{
      "version": 1,
      "events": [
        {"state": "arriving", "attention": 0.3, "t": 0.0},
        {"state": "listening", "attention": 1, "t": 2.0},
        {"state": "thinking", "attention": 0.6, "t": 9.5},
        {"state": "speaking", "attention": 0.85, "t": 9.5}
      ]
    }"#;

    fn states(due: &[TimedState]) -> Vec<&str> {
        due.iter()
            .map(|e| e.state["state"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn events_play_once_each_when_their_time_comes() {
        let mut t = ExpressionTimeline::parse(ONE_TURN).unwrap();
        assert_eq!(t.len(), 4);
        assert_eq!(states(t.due(0.0)), ["arriving"]);
        assert!(t.due(1.9).is_empty());
        assert_eq!(states(t.due(2.0)), ["listening"]);
        assert!(t.due(2.0).is_empty(), "already played");
        assert!(!t.is_finished());
        // one long frame past two events at the same time: both, in file order
        assert_eq!(states(t.due(30.0)), ["thinking", "speaking"]);
        assert!(t.is_finished());
        assert!(t.due(60.0).is_empty());
    }

    #[test]
    fn a_clock_of_summed_steps_reaches_a_whole_time_on_its_frame() {
        let mut t = ExpressionTimeline::parse(ONE_TURN).unwrap();
        t.due(0.0);
        let mut clock = 0.0_f64;
        for _ in 0..120 {
            clock += 1.0 / 60.0;
        }
        assert!(clock < 2.0, "the sum this guards against: {clock}");
        assert_eq!(states(t.due(clock)), ["listening"]);
        // A frame short is still short.
        let mut t = ExpressionTimeline::parse(ONE_TURN).unwrap();
        t.due(0.0);
        assert!(t.due(2.0 - 1.0 / 240.0).is_empty());
    }

    #[test]
    fn bad_timelines_say_which_event() {
        let bad = |src: &str| ExpressionTimeline::parse(src).unwrap_err();
        assert!(bad("[]").contains("object"));
        assert!(bad(r#"{"version": 1}"#).contains("events"));
        assert!(bad(r#"{"events": [3]}"#).contains("event 0: not an object"));
        assert!(bad(r#"{"events": [{"state": "x"}]}"#).contains("event 0: no numeric `t`"));
        assert!(bad(r#"{"events": [{"t": -1}]}"#).contains("0 or later"));
        assert!(bad(r#"{"events": [{"t": 2}, {"t": 1}]}"#).contains("event 1"));
        assert!(bad("{").contains("EOF"));
    }

    #[test]
    fn numbers_reach_scripts_as_floats() {
        let mut t = ExpressionTimeline::parse(ONE_TURN).unwrap();
        t.due(2.0);
        let map = json_to_map(&t.events[1].state);
        assert_eq!(
            map["attention"].as_float().unwrap(),
            1.0,
            "an integer 1 is 1.0"
        );
        assert_eq!(map["t"].as_float().unwrap(), 2.0);
        assert_eq!(map["state"].clone().into_string().unwrap(), "listening");
        let nested = serde_json::json!({"a": [1, null, {"b": true}]});
        let d = json_to_dynamic(&nested);
        let a = d.cast::<Map>()["a"].clone().into_array().unwrap();
        assert_eq!(a[0].as_float().unwrap(), 1.0);
        assert!(a[1].is_unit());
        assert!(a[2].clone().cast::<Map>()["b"].as_bool().unwrap());
    }

    #[test]
    fn socket_lines_must_be_objects() {
        assert!(parse_line(br#"{"state": "settled"}"#).is_ok());
        assert!(parse_line(b"42").unwrap_err().contains("a number"));
        assert!(parse_line(b"{oops").unwrap_err().contains("not JSON"));
    }

    #[test]
    fn a_feed_with_neither_source_is_none() {
        assert!(ExpressionFeed::open(None, None).unwrap().is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_socket_is_checked_at_start_and_bound_once_frames_run() {
        // A malformed or non-loopback address still fails the run at start
        assert!(ExpressionFeed::open(None, Some("0.0.0.0:0")).is_err());
        assert!(ExpressionFeed::open(None, Some("localhost:7777")).is_err());

        // A taken port is known when the socket opens, and is the run's
        // failure (#584)
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = busy.local_addr().unwrap().to_string();
        let mut taken = ExpressionFeed::open(None, Some(&busy)).unwrap().unwrap();
        assert!(taken.failure().is_none());
        assert!(taken.open_socket(SOCKET_OPENS_AT_FRAME, 0.25).is_none());
        let why = taken.failure().expect("the bind's failure");
        assert!(why.contains("--expression-listen"), "{why}");

        let mut feed = ExpressionFeed::open(None, Some("127.0.0.1:0"))
            .unwrap()
            .unwrap();
        assert!(feed.listener.is_none(), "not bound before the first frame");
        let waiting = feed.socket_counts().unwrap();
        assert!(!waiting.open, "{waiting:?}");

        let addr = feed.open_socket(SOCKET_OPENS_AT_FRAME, 0.25).unwrap();
        assert_ne!(addr.port(), 0, "port 0 picked one");
        let open = feed.socket_counts().unwrap();
        assert!(open.open && open.addr == addr, "{open:?}");
        assert!(feed.open_socket(3, 0.5).is_none(), "it opens once");
        std::net::TcpStream::connect(addr).expect("it accepts now");
    }
}
