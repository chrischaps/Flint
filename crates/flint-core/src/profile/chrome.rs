//! The recorder's file: Chrome trace-event JSON (ADR 0128), which Perfetto
//! and `chrome://tracing` open with no tooling of Flint's.
//!
//! One `"ph":"X"` (complete) event per scope, with `args: { frame, depth,
//! detail? }`, or `{ frame, depth, calls, worst_us }` for a summed record
//! (#1288: one event standing for `calls` calls of a function, laid from
//! the first for their total, a leaf that may overlap its siblings); one
//! track per thread ring, and any number of a host's own
//! tracks (the player's frames, phases, GPU passes, hitches), each a `tid`
//! of process 1 with a `thread_name` and a `thread_sort_index`. Times are
//! microseconds from the recorder's epoch (`clock`), to the nanosecond.
//! The header is `otherData`, the format's place for a trace's metadata,
//! which Perfetto shows under "Info and stats".
//!
//! ```
//! use flint_core::profile::chrome::{Arg, ChromeTrace};
//! let mut trace = ChromeTrace::new();
//! let frames = trace.track("frames");
//! trace.complete(frames, "frame", 1_000, 16_000_000, &[("ms", Arg::Num(16.0))]);
//! let json = trace.finish(&[("scene", Arg::Str("tavern.scene.toml"))]);
//! assert!(json.contains("\"traceEvents\""));
//! ```

use super::{TakenDetail, ThreadScopes};
use std::fmt::Write as _;

/// A value in an event's `args` or the header.
#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    Str(&'a str),
    /// Written with up to three decimals; NaN and infinities as `null`.
    Num(f64),
    Int(i64),
    Bool(bool),
}

/// A trace being written; [`finish`](Self::finish) gives the file's text.
pub struct ChromeTrace {
    out: String,
    first: bool,
    next_tid: u32,
}

impl Default for ChromeTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl ChromeTrace {
    pub fn new() -> Self {
        let mut out = String::with_capacity(64 * 1024);
        out.push_str("{\"displayTimeUnit\":\"ms\",\"traceEvents\":[\n");
        Self {
            out,
            first: true,
            next_tid: 1,
        }
    }

    /// A new track named `name`, shown in the order tracks are made.
    pub fn track(&mut self, name: &str) -> u32 {
        let tid = self.next_tid;
        self.next_tid += 1;
        self.begin_event();
        self.out
            .push_str("{\"ph\":\"M\",\"name\":\"thread_name\",\"pid\":1,\"tid\":");
        let _ = write!(self.out, "{tid},\"args\":{{\"name\":");
        push_str(&mut self.out, name);
        self.out.push_str("}}");
        self.begin_event();
        let _ = write!(
            self.out,
            "{{\"ph\":\"M\",\"name\":\"thread_sort_index\",\"pid\":1,\"tid\":{tid},\"args\":{{\"sort_index\":{tid}}}}}"
        );
        tid
    }

    /// Names the one process the tracks are in (Perfetto's group title).
    pub fn process(&mut self, name: &str) {
        self.begin_event();
        self.out
            .push_str("{\"ph\":\"M\",\"name\":\"process_name\",\"pid\":1,\"args\":{\"name\":");
        push_str(&mut self.out, name);
        self.out.push_str("}}");
    }

    /// An event `start_ns` to `start_ns + dur_ns` on `tid`.
    pub fn complete(
        &mut self,
        tid: u32,
        name: &str,
        start_ns: u64,
        dur_ns: u64,
        args: &[(&str, Arg)],
    ) {
        self.begin_event();
        self.out.push_str("{\"ph\":\"X\",\"name\":");
        push_str(&mut self.out, name);
        let _ = write!(self.out, ",\"pid\":1,\"tid\":{tid},\"ts\":");
        push_us(&mut self.out, start_ns);
        self.out.push_str(",\"dur\":");
        push_us(&mut self.out, dur_ns);
        push_args(&mut self.out, args);
        self.out.push('}');
    }

    /// A moment at `at_ns` on `tid` (an instant event, scoped to its track).
    pub fn instant(&mut self, tid: u32, name: &str, at_ns: u64, args: &[(&str, Arg)]) {
        self.begin_event();
        self.out.push_str("{\"ph\":\"i\",\"s\":\"t\",\"name\":");
        push_str(&mut self.out, name);
        let _ = write!(self.out, ",\"pid\":1,\"tid\":{tid},\"ts\":");
        push_us(&mut self.out, at_ns);
        push_args(&mut self.out, args);
        self.out.push('}');
    }

    /// A thread's scopes as a track of their own, named for the thread.
    pub fn scopes(&mut self, thread: &ThreadScopes) -> u32 {
        let tid = self.track(&thread.thread);
        for s in &thread.scopes {
            let frame = ("frame", Arg::Int(s.frame as i64));
            let depth = ("depth", Arg::Int(s.depth as i64));
            match &s.detail {
                None => self.complete(tid, s.name, s.start_ns, s.dur_ns, &[frame, depth]),
                Some(TakenDetail::Count(n)) => self.complete(
                    tid,
                    s.name,
                    s.start_ns,
                    s.dur_ns,
                    &[frame, depth, ("detail", Arg::Int(*n as i64))],
                ),
                Some(TakenDetail::Text(t)) => self.complete(
                    tid,
                    s.name,
                    s.start_ns,
                    s.dur_ns,
                    &[frame, depth, ("detail", Arg::Str(t))],
                ),
                // A summed record (#1288): `calls` calls laid from the
                // first, `dur` their total, `worst_us` the longest
                Some(TakenDetail::Calls { calls, worst_ns }) => self.complete(
                    tid,
                    s.name,
                    s.start_ns,
                    s.dur_ns,
                    &[
                        frame,
                        depth,
                        ("calls", Arg::Int(*calls as i64)),
                        ("worst_us", Arg::Num(*worst_ns as f64 / 1000.0)),
                    ],
                ),
            }
        }
        tid
    }

    /// The file's text, with `header` as its `otherData`.
    pub fn finish(mut self, header: &[(&str, Arg)]) -> String {
        self.out.push_str("\n],\"otherData\":");
        push_object(&mut self.out, header);
        self.out.push_str("}\n");
        self.out
    }

    /// Bytes written so far.
    pub fn len(&self) -> usize {
        self.out.len()
    }

    pub fn is_empty(&self) -> bool {
        self.first
    }

    fn begin_event(&mut self) {
        if !self.first {
            self.out.push_str(",\n");
        }
        self.first = false;
    }
}

fn push_args(out: &mut String, args: &[(&str, Arg)]) {
    if args.is_empty() {
        return;
    }
    out.push_str(",\"args\":");
    push_object(out, args);
}

fn push_object(out: &mut String, fields: &[(&str, Arg)]) {
    out.push('{');
    for (i, (key, value)) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_str(out, key);
        out.push(':');
        match *value {
            Arg::Str(s) => push_str(out, s),
            Arg::Num(n) if n.is_finite() => {
                let _ = write!(out, "{}", (n * 1000.0).round() / 1000.0);
            }
            Arg::Num(_) => out.push_str("null"),
            Arg::Int(n) => {
                let _ = write!(out, "{n}");
            }
            Arg::Bool(b) => out.push_str(if b { "true" } else { "false" }),
        }
    }
    out.push('}');
}

/// Nanoseconds as microseconds with three decimals: the nanosecond, exact.
fn push_us(out: &mut String, ns: u64) {
    let _ = write!(out, "{}.{:03}", ns / 1000, ns % 1000);
}

/// `s` as a JSON string.
fn push_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::TakenScope;

    #[test]
    fn a_trace_is_json_with_its_tracks_events_and_header() {
        let mut trace = ChromeTrace::new();
        let thread = ThreadScopes {
            thread: "main \"thread\"".into(),
            scopes: vec![TakenScope {
                name: "tick",
                file: "frame.rs",
                line: 1,
                start_ns: 1_234_567,
                dur_ns: 16_000_001,
                frame: 7,
                depth: 0,
                detail: Some(TakenDetail::Text("models/a\\b.glb\n".into())),
            }],
            ..Default::default()
        };
        trace.scopes(&thread);
        let hitches = trace.track("hitches");
        trace.instant(hitches, "hitch", 5_000, &[("ms", Arg::Num(f64::NAN))]);
        let text = trace.finish(&[("scene", Arg::Str("a.scene.toml")), ("frames", Arg::Int(3))]);
        assert!(text.contains(r#""ts":1234.567,"dur":16000.001"#), "{text}");
        assert!(text.contains(r#""name":"main \"thread\"""#), "{text}");
        assert!(text.contains(r#""detail":"models/a\\b.glb\n""#), "{text}");
        assert!(text.contains(r#""ms":null"#), "{text}");
        assert!(
            text.contains(r#""otherData":{"scene":"a.scene.toml","frames":3}"#),
            "{text}"
        );
        // Every line between the brackets is one event
        let events = text
            .lines()
            .filter(|l| l.starts_with('{') && l.contains("\"ph\""))
            .count();
        assert_eq!(
            events, 6,
            "two metadata events a track, the scope and the hitch: {text}"
        );
    }
}
