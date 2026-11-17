//! `flint profile explain`: why one frame was slow, as a sentence first.
//!
//! The frame is held against the trace's normal frames
//! ([`stats::normal_frames`]): each scope key's time in it against that
//! key's median there, a frame it didn't run in counting 0. The answer is
//! found by walking down the frame thread's tree from the frame's excess:
//! at each level the child with the largest excess is taken while it
//! carries at least half of its parent's ([`CARRIES`]), so the walk stops
//! at the deepest scope that still explains the frame, not at `tick`,
//! which every frame's excess is inside. When no outermost scope carries
//! it, the time was outside any scope (a wait in `acquire` or `present`),
//! and the phases say which.

use super::frame_tree::{tree, Node};
use super::read::Trace;
use super::stats::{self, ms, round, signed};
use super::{about, about_json, pick_frame, ExplainArgs};
use crate::commands::frame::table::Table;
use crate::output::{self, Answer};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;

/// How much of its parent's excess a scope must carry for the walk to go
/// into it.
pub(crate) const CARRIES: f64 = 0.5;

/// Excess under this many ms is noise, whatever its share.
const FLOOR_MS: f64 = 0.25;

/// Rows under this many ms of excess aren't listed.
const LIST_MS: f64 = 0.05;

pub fn run(args: ExplainArgs) -> Result<()> {
    let trace = Trace::open(&args.trace)?;
    let (text, doc) = explain(&trace, &args.frame, args.limit)?;
    output::emit(&Answer::new(text, doc))
}

/// One key in the frame against the normal frames.
#[derive(Clone)]
struct Against {
    key: String,
    this_ms: f64,
    self_ms: f64,
    calls: u32,
    median_ms: f64,
    median_self_ms: f64,
    /// Normal frames it ran in.
    ran_in: usize,
}

impl Against {
    fn excess(&self) -> f64 {
        self.this_ms - self.median_ms
    }
    fn self_excess(&self) -> f64 {
        self.self_ms - self.median_self_ms
    }
}

pub(crate) fn explain(trace: &Trace, which: &str, limit: usize) -> Result<(String, Value)> {
    let pos = pick_frame(trace, which)?;
    let span = &trace.frames[pos];
    let normal = stats::normal_frames(trace, Some(pos));
    let n = normal.positions.len();
    let all: Vec<_> = trace.threads.iter().collect();
    let per = stats::per_frame(trace, &all, true);
    let incl = stats::values(&per, &normal.positions, |k| k.incl_ms());
    let selfs = stats::values(&per, &normal.positions, |k| k.self_ms());
    let empty = Vec::new();

    let mut keys: Vec<Against> = per[pos]
        .iter()
        .map(|(key, k)| {
            let i = incl.get(key.as_str()).unwrap_or(&empty);
            let s = selfs.get(key.as_str()).unwrap_or(&empty);
            Against {
                key: key.clone(),
                this_ms: k.incl_ms(),
                self_ms: k.self_ms(),
                calls: k.calls,
                median_ms: stats::quantile(i, n, 0.5),
                median_self_ms: stats::quantile(s, n, 0.5),
                ran_in: i.len(),
            }
        })
        .collect();
    let by_key: HashMap<String, usize> = keys
        .iter()
        .enumerate()
        .map(|(i, a)| (a.key.clone(), i))
        .collect();

    let frame_ms = span.ms();
    let excess = frame_ms - normal.median_ms;

    // The frame thread: the one whose outermost scopes hold the most time
    let mut roots: Vec<(usize, Node)> = trace
        .threads
        .iter()
        .enumerate()
        .map(|(i, t)| (i, tree(t, span.frame)))
        .collect();
    roots.sort_by_key(|r| std::cmp::Reverse(r.1.incl_ns));
    let (frame_thread, root) = roots.swap_remove(0);
    let node_excess = |node: &Node| {
        let median = by_key.get(&node.key).map_or(0.0, |&i| keys[i].median_ms);
        node.incl_ns as f64 / 1e6 - median
    };
    let mut path: Vec<&Node> = Vec::new();
    let mut carried = excess;
    let mut at = &root;
    while let Some(best) = at
        .children
        .iter()
        .max_by(|a, b| node_excess(a).total_cmp(&node_excess(b)))
    {
        let e = node_excess(best);
        if e < FLOOR_MS || e < carried * CARRIES {
            break;
        }
        path.push(best);
        carried = e;
        at = best;
    }
    let culprit: Option<Against> = path.last().map(|node| keys[by_key[&node.key]].clone());
    let culprit = culprit.as_ref();

    // The phases against theirs
    let phase_names = trace.phase_order();
    let normal_phases: Vec<_> = normal
        .positions
        .iter()
        .map(|&p| trace.phases_of(p))
        .collect();
    let this_phases = trace.phases_of(pos);
    let mut phases: Vec<(String, f64, f64)> = phase_names
        .iter()
        .map(|p| {
            let values: Vec<f64> = normal_phases
                .iter()
                .filter_map(|m| m.get(p).copied())
                .collect();
            let median = stats::quantile(&values, n, 0.5);
            (
                p.clone(),
                this_phases.get(p).copied().unwrap_or(0.0),
                median,
            )
        })
        .collect();
    let worst_phase = phases
        .iter()
        .max_by(|a, b| (a.1 - a.2).total_cmp(&(b.1 - b.2)))
        .cloned();

    // The answer, first
    let mut answer = format!(
        "frame {} took {} ms (median {})",
        span.frame,
        ms(frame_ms),
        ms(normal.median_ms)
    );
    let mut said = Vec::new();
    if excess < FLOOR_MS {
        answer.push_str(": no slower than a normal frame, nothing to explain");
    } else if let Some(c) = culprit {
        if c.ran_in == 0 {
            answer.push_str(&format!(": {} {} ms ran only here", c.key, ms(c.this_ms)));
        } else {
            answer.push_str(&format!(
                ": {} {} ms against its median {}",
                c.key,
                ms(c.this_ms),
                ms(c.median_ms)
            ));
        }
        answer.push_str(&format!(
            ", {} of the frame's {} ms excess",
            signed(c.excess()),
            signed(excess)
        ));
        if path.len() > 1 {
            let chain: Vec<&str> = path[..path.len() - 1]
                .iter()
                .map(|n| n.key.as_str())
                .collect();
            said.push(format!(
                "inside {} on {}",
                chain.join(" > "),
                trace.threads[frame_thread].name
            ));
        }
        if c.self_excess() < c.excess() * CARRIES {
            said.push(format!(
                "most of it is spread over {}'s children, none carrying half: `flint profile frame {} {}` shows them",
                c.key, trace.path, span.frame
            ));
        }
    } else if let Some((phase, this, median)) = &worst_phase {
        answer.push_str(&format!(
            ": no scope covers the {} ms excess; the {phase} phase took {} ms against its median {}",
            signed(excess),
            ms(*this),
            ms(*median)
        ));
        if phase == "acquire" || phase == "present" {
            said.push(
                "a wait, not work: for the GPU or the display (the hitch detector's `elsewhere`)"
                    .into(),
            );
        }
    } else {
        answer.push_str(&format!(
            ": no scope covers the {} ms excess",
            signed(excess)
        ));
    }

    let hitch = trace
        .hitch_frames()
        .iter()
        .position(|h| *h == Some(pos))
        .map(|i| &trace.hitches[i]);

    let mut text = answer.clone();
    text.push('\n');
    for line in &said {
        text.push_str(&format!("  {line}\n"));
    }
    if let Some(h) = hitch {
        text.push_str(&format!(
            "  the hitch detector: {}{}\n",
            h.cause,
            match (h.judged_ms, h.median_ms) {
                (Some(j), Some(m)) => format!(" (judged {} ms against {})", ms(j), ms(m)),
                _ => String::new(),
            }
        ));
    }
    text.push('\n');
    text.push_str(&about(trace));
    text.push_str(&format!("\n  held against {}\n", normal.rule));

    phases.retain(|(_, this, median)| (this - median).abs() >= LIST_MS);
    phases.sort_by(|a, b| (b.1 - b.2).total_cmp(&(a.1 - a.2)));
    if !phases.is_empty() {
        let mut table = Table::new(&["phase", ">this ms", ">median", ">excess"]);
        for (p, this, median) in &phases {
            table.row(vec![
                p.clone(),
                ms(*this),
                ms(*median),
                signed(this - median),
            ]);
        }
        text.push_str("\nphases against their medians:\n");
        text.push_str(&table.render("  "));
    }

    keys.sort_by(|a, b| {
        b.self_excess()
            .total_cmp(&a.self_excess())
            .then(b.excess().total_cmp(&a.excess()))
            .then(a.key.cmp(&b.key))
    });
    let listed: Vec<&Against> = keys
        .iter()
        .filter(|a| a.excess() >= LIST_MS || a.self_excess() >= LIST_MS)
        .take(limit)
        .collect();
    if !listed.is_empty() {
        let mut table = Table::new(&[
            "scope",
            ">this ms",
            ">median",
            ">excess",
            ">self excess",
            ">calls",
            ">ran in",
        ]);
        for a in &listed {
            table.row(vec![
                a.key.clone(),
                ms(a.this_ms),
                ms(a.median_ms),
                signed(a.excess()),
                signed(a.self_excess()),
                a.calls.to_string(),
                format!("{}/{n}", a.ran_in),
            ]);
        }
        text.push_str("\nscopes by self excess (the time in a scope, less its children's, over its median):\n");
        text.push_str(&table.render("  "));
    }
    let mut only: Vec<&Against> = keys
        .iter()
        .filter(|a| a.ran_in == 0 && a.this_ms >= LIST_MS)
        .collect();
    only.sort_by(|a, b| b.this_ms.total_cmp(&a.this_ms));
    only.truncate(limit);
    if !only.is_empty() {
        let mut table = Table::new(&["scope", ">ms", ">calls"]);
        for a in &only {
            table.row(vec![a.key.clone(), ms(a.this_ms), a.calls.to_string()]);
        }
        text.push_str("\nran in this frame and in no normal one:\n");
        text.push_str(&table.render("  "));
    }
    let notes = trace.notes_of(pos);
    if !notes.is_empty() {
        text.push_str("\nframe notes:\n");
        for note in &notes {
            text.push_str(&format!(
                "  {} {}{}\n",
                note.kind,
                note.detail,
                note.ms
                    .map(|m| format!(" {} ms", ms(m)))
                    .unwrap_or_default()
            ));
        }
    }

    let row = |a: &Against| {
        json!({
            "scope": a.key,
            "ms": round(a.this_ms),
            "median_ms": round(a.median_ms),
            "excess_ms": round(a.excess()),
            "self_ms": round(a.self_ms),
            "median_self_ms": round(a.median_self_ms),
            "self_excess_ms": round(a.self_excess()),
            "calls": a.calls,
            "normal_frames_ran_in": a.ran_in,
        })
    };
    let doc = json!({
        "trace": about_json(trace),
        "answer": answer,
        "frame": {
            "frame": span.frame,
            "ms": round(frame_ms),
            "median_ms": round(normal.median_ms),
            "excess_ms": round(excess),
            "normal_frames": n,
            "normal_rule": normal.rule,
        },
        "culprit": culprit.filter(|_| excess >= FLOOR_MS).map(|c| {
            let mut v = row(c);
            v["only_here"] = json!(c.ran_in == 0);
            v["path"] = json!(path.iter().map(|n| n.key.as_str()).collect::<Vec<_>>());
            v["thread"] = json!(trace.threads[frame_thread].name);
            v
        }),
        "notes_said": said,
        "hitch": hitch.map(|h| json!({
            "cause": h.cause, "judged_ms": h.judged_ms, "median_ms": h.median_ms, "game_time": h.game_time
        })),
        "phases": phases.iter().map(|(p, this, median)| json!({
            "phase": p, "ms": round(*this), "median_ms": round(*median), "excess_ms": round(this - median)
        })).collect::<Vec<_>>(),
        "scopes": listed.iter().map(|a| row(a)).collect::<Vec<_>>(),
        "only_here": only.iter().map(|a| row(a)).collect::<Vec<_>>(),
        "frame_notes": notes.iter().map(|n| json!({"kind": n.kind, "detail": n.detail, "ms": n.ms})).collect::<Vec<_>>(),
    });
    Ok((text, doc))
}
