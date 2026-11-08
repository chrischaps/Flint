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
//!   loopback only and is off unless the flag is given. Reader threads parse
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

use rhai::{Dynamic, Map};
use serde_json::Value;

/// The longest line the socket takes, in bytes, newline excluded.
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_LINE_BYTES: usize = 64 * 1024;
/// The most socket messages a frame takes. The rest wait for the next one.
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_PER_FRAME: usize = 64;
/// The most socket messages held for the frame. Past this the oldest go, so a
/// flood never costs the latest state, which is the one that matters.
#[cfg(not(target_arch = "wasm32"))]
pub const QUEUE_CAPACITY: usize = 256;
/// The most connections the socket keeps open at once.
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_CONNECTIONS: usize = 8;

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
    use super::*;
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader, Read};
    use std::net::{SocketAddr, TcpListener, TcpStream};
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
        /// Set when the listener is dropped, so readers stop.
        closed: AtomicBool,
    }

    impl Shared {
        fn push(&self, state: State) {
            let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            if q.len() >= QUEUE_CAPACITY {
                q.pop_front();
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            q.push_back(state);
        }
    }

    /// A loopback TCP listener taking one JSON state per line, read off the
    /// frame thread.
    pub struct ExpressionListener {
        shared: Arc<Shared>,
        addr: SocketAddr,
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
            let wanted: SocketAddr = addr.parse().map_err(|e| {
                anyhow::anyhow!(
                    "--expression-listen {addr}: expected IP:port, like 127.0.0.1:7777 ({e})"
                )
            })?;
            if !wanted.ip().is_loopback() {
                anyhow::bail!(
                    "--expression-listen {addr}: only a loopback address (127.0.0.1 or ::1) can listen"
                );
            }
            let listener = TcpListener::bind(wanted)
                .map_err(|e| anyhow::anyhow!("--expression-listen {addr}: {e}"))?;
            let addr = listener.local_addr()?;
            let shared = Arc::new(Shared::default());
            let for_thread = shared.clone();
            std::thread::Builder::new()
                .name("expression-listen".into())
                .spawn(move || accept_loop(listener, for_thread))?;
            tracing::info!(target: "expression", "listening on {addr}");
            Ok(Self { shared, addr })
        }

        /// Where it's listening (the real port when bound to port 0).
        pub fn local_addr(&self) -> SocketAddr {
            self.addr
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
    }

    fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
        let open = Arc::new(AtomicUsize::new(0));
        for stream in listener.incoming() {
            if shared.closed.load(Ordering::Relaxed) {
                return;
            }
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(target: "expression", "accept failed: {e}");
                    continue;
                }
            };
            let peer = stream
                .peer_addr()
                .map(|a| a.to_string())
                .unwrap_or_else(|_| "?".into());
            if open.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                tracing::warn!(
                    target: "expression",
                    "refused {peer}: already {MAX_CONNECTIONS} connections"
                );
                continue;
            }
            open.fetch_add(1, Ordering::SeqCst);
            let shared = shared.clone();
            let still_open = open.clone();
            let spawned = std::thread::Builder::new()
                .name("expression-reader".into())
                .spawn(move || {
                    tracing::info!(target: "expression", "{peer} connected");
                    read_lines(stream, &shared, &peer);
                    tracing::info!(target: "expression", "{peer} disconnected");
                    still_open.fetch_sub(1, Ordering::SeqCst);
                });
            if let Err(e) = spawned {
                open.fetch_sub(1, Ordering::SeqCst);
                tracing::warn!(target: "expression", "no reader thread: {e}");
            }
        }
    }

    /// Read lines until the peer hangs up or the player goes away.
    fn read_lines(stream: TcpStream, shared: &Shared, peer: &str) {
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        while !shared.closed.load(Ordering::Relaxed) {
            line.clear();
            match read_capped_line(&mut reader, &mut line) {
                Ok(Line::Eof) => return,
                Ok(Line::TooLong) => {
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
                Ok(state) => shared.push(state),
                Err(why) => {
                    tracing::warn!(target: "expression", "dropped a line from {peer}: {why}");
                }
            }
        }
    }

    enum Line {
        Ready,
        TooLong,
        Eof,
    }

    /// Read one line into `buf` (newline excluded), holding at most
    /// [`MAX_LINE_BYTES`] of it. A longer line is read through to its end and
    /// thrown away. A last line without a newline still counts.
    fn read_capped_line<R: BufRead>(reader: &mut R, buf: &mut Vec<u8>) -> std::io::Result<Line> {
        let n = reader
            .by_ref()
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', buf)?;
        if n == 0 {
            return Ok(Line::Eof);
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
            return Ok(Line::Ready);
        }
        if buf.len() <= MAX_LINE_BYTES {
            return Ok(Line::Ready); // the stream ended mid-line
        }
        // Too long: skip to the end of it without holding any more.
        loop {
            let (done, used) = {
                let chunk = reader.fill_buf()?;
                if chunk.is_empty() {
                    return Ok(Line::TooLong);
                }
                match chunk.iter().position(|&b| b == b'\n') {
                    Some(i) => (true, i + 1),
                    None => (false, chunk.len()),
                }
            };
            reader.consume(used);
            if done {
                return Ok(Line::TooLong);
            }
        }
    }

    fn trim(bytes: &[u8]) -> &[u8] {
        let start = bytes
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(bytes.len());
        let end = bytes
            .iter()
            .rposition(|b| !b.is_ascii_whitespace())
            .map_or(start, |i| i + 1);
        &bytes[start..end]
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
        }
    }
}

/// Every expression source the player was started with.
#[derive(Default)]
pub struct ExpressionFeed {
    pub timeline: Option<ExpressionTimeline>,
    #[cfg(not(target_arch = "wasm32"))]
    pub listener: Option<ExpressionListener>,
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
            println!("Expression timeline: {path} ({} events)", t.len());
            feed.timeline = Some(t);
        }
        if let Some(addr) = listen {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let l = ExpressionListener::bind(addr)?;
                println!(
                    "Expression listening on {} (one JSON object per line)",
                    l.local_addr()
                );
                feed.listener = Some(l);
            }
            #[cfg(target_arch = "wasm32")]
            anyhow::bail!("--expression-listen {addr}: no sockets on the web");
        }
        Ok(Some(feed))
    }

    /// The states due this frame at game time `now`: the timeline's first,
    /// then whatever the socket has, at most [`MAX_PER_FRAME`]. Each is logged
    /// at info under the `expression` target.
    pub fn poll(&mut self, now: f64) -> Vec<Map> {
        let mut out = Vec::new();
        if let Some(timeline) = &mut self.timeline {
            for event in timeline.due(now) {
                log_state("timeline", now, &event.state);
                out.push(json_to_map(&event.state));
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(listener) = &self.listener {
            for state in listener.drain(MAX_PER_FRAME) {
                log_state("socket", now, &state);
                out.push(json_to_map(&state));
            }
        }
        out
    }
}

fn log_state(source: &str, now: f64, state: &serde_json::Map<String, Value>) {
    if tracing::enabled!(target: "expression", tracing::Level::INFO) {
        let json = serde_json::to_string(state).unwrap_or_default();
        tracing::info!(target: "expression", "{now:8.3}s {source}: {json}");
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
}
