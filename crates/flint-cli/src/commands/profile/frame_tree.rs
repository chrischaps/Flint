//! `flint profile frame`: one frame's scope tree, thread by thread, as an
//! indented table, siblings of one name merged, short scopes left out.

use super::read::{Detail, Thread, Trace};
use super::stats::{self, ms, round};
use super::{about, about_json, pick_frame, FrameArgs};
use crate::commands::frame::table::Table;
use crate::output::{self, Answer};
use anyhow::Result;
use serde_json::{json, Value};

pub fn run(args: FrameArgs) -> Result<()> {
    let trace = Trace::open(&args.trace)?;
    let (text, doc) = view(&trace, &args)?;
    output::emit(&Answer::new(text, doc))
}

/// A scope key under its parent, the calls of that key there merged.
#[derive(Default)]
pub(crate) struct Node {
    pub key: String,
    pub incl_ns: u64,
    pub self_ns: u64,
    pub calls: u32,
    /// A count detail, when one call carried one.
    pub count: Option<i64>,
    pub children: Vec<Node>,
    /// Children left out below the threshold: how many, and their time.
    pub pruned: (usize, u64),
}

impl Node {
    fn child(&mut self, key: &str) -> &mut Node {
        if let Some(i) = self.children.iter().position(|c| c.key == key) {
            return &mut self.children[i];
        }
        self.children.push(Node {
            key: key.to_string(),
            ..Default::default()
        });
        self.children.last_mut().unwrap()
    }

    fn prune(&mut self, min_ns: u64) {
        let (kept, cut): (Vec<Node>, Vec<Node>) = std::mem::take(&mut self.children)
            .into_iter()
            .partition(|c| c.incl_ns >= min_ns);
        self.pruned = (cut.len(), cut.iter().map(|c| c.incl_ns).sum());
        self.children = kept;
        for c in &mut self.children {
            c.prune(min_ns);
        }
    }
}

/// The tree of `thread`'s scopes that closed in `frame`, under a root
/// holding their outermost time.
pub(crate) fn tree(thread: &Thread, frame: u32) -> Node {
    let mut root = Node::default();
    for (i, s) in thread.scopes.iter().enumerate() {
        if s.frame != frame {
            continue;
        }
        let mut node = &mut root;
        for key in thread.ancestors(i, true) {
            node = node.child(&key);
        }
        let node = node.child(&s.key(true));
        node.incl_ns += s.dur_ns;
        node.self_ns += thread.self_ns[i];
        node.calls += s.calls();
        node.count = match (&s.detail, node.calls) {
            (Some(Detail::Count(n)), 1) => Some(*n),
            _ => None,
        };
        if s.depth == 0 {
            root.incl_ns += s.dur_ns;
        }
    }
    root
}

fn view(trace: &Trace, args: &FrameArgs) -> Result<(String, Value)> {
    let threads = trace.threads_named(args.thread.as_deref())?;
    let pos = pick_frame(trace, &args.frame)?;
    let span = &trace.frames[pos];
    let normal = stats::normal_frames(trace, Some(pos));
    let frame_ms = span.ms();
    let min_ns = (args.min_ms.max(0.0) * 1e6) as u64;
    let share = |ns: u64| {
        if frame_ms > 0.0 {
            ns as f64 / 1e6 / frame_ms * 100.0
        } else {
            0.0
        }
    };

    let mut text = about(trace);
    text.push_str(&format!(
        "\nframe {}: {} ms (median {} over {} normal frames)\n",
        span.frame,
        ms(frame_ms),
        ms(normal.median_ms),
        normal.positions.len()
    ));
    let hitch = trace
        .hitch_frames()
        .iter()
        .position(|h| *h == Some(pos))
        .map(|i| &trace.hitches[i]);
    if let Some(h) = hitch {
        text.push_str(&format!("hitch: {}\n", h.cause));
    }

    let mut json_threads = Vec::new();
    for t in &threads {
        let mut root = tree(t, span.frame);
        if root.children.is_empty() {
            continue;
        }
        root.prune(min_ns);
        text.push_str(&format!(
            "\n{}: {} ms in its outermost scopes\n",
            t.name,
            ms(root.incl_ns as f64 / 1e6)
        ));
        let mut table = Table::new(&["scope", ">ms", ">share", ">self ms", ">calls"]);
        fn rows(
            node: &Node,
            depth: usize,
            table: &mut Table,
            share: &dyn Fn(u64) -> f64,
            min_ns: u64,
        ) -> Vec<Value> {
            let mut out = Vec::new();
            for c in &node.children {
                let mut name = format!("{}{}", "  ".repeat(depth), c.key);
                if let Some(n) = c.count {
                    name.push_str(&format!(" [{n}]"));
                }
                table.row(vec![
                    name,
                    ms(c.incl_ns as f64 / 1e6),
                    format!("{:.1}%", share(c.incl_ns)),
                    ms(c.self_ns as f64 / 1e6),
                    c.calls.to_string(),
                ]);
                let children = rows(c, depth + 1, table, share, min_ns);
                let mut v = json!({
                    "scope": c.key,
                    "ms": round(c.incl_ns as f64 / 1e6),
                    "share": round(share(c.incl_ns) / 100.0),
                    "self_ms": round(c.self_ns as f64 / 1e6),
                    "calls": c.calls,
                    "children": children,
                });
                if let Some(n) = c.count {
                    v["count"] = json!(n);
                }
                if c.pruned.0 > 0 {
                    v["pruned"] =
                        json!({ "scopes": c.pruned.0, "ms": round(c.pruned.1 as f64 / 1e6) });
                }
                out.push(v);
            }
            if node.pruned.0 > 0 && node.pruned.1 >= min_ns {
                table.row(vec![
                    format!(
                        "{}({} more under the threshold)",
                        "  ".repeat(depth),
                        node.pruned.0
                    ),
                    ms(node.pruned.1 as f64 / 1e6),
                    format!("{:.1}%", share(node.pruned.1)),
                    String::new(),
                    String::new(),
                ]);
            }
            out
        }
        let tree_json = rows(&root, 0, &mut table, &share, min_ns);
        text.push_str(&table.render("  "));
        let mut thread_json = json!({
            "thread": t.name,
            "ms": round(root.incl_ns as f64 / 1e6),
            "scopes": tree_json,
        });
        if root.pruned.0 > 0 {
            thread_json["pruned"] =
                json!({ "scopes": root.pruned.0, "ms": round(root.pruned.1 as f64 / 1e6) });
        }
        json_threads.push(thread_json);
    }
    if json_threads.is_empty() {
        text.push_str("\nno scope closed in this frame on the threads asked for\n");
    }

    let phases = trace.phases_of(pos);
    let mut phase_json = serde_json::Map::new();
    if !phases.is_empty() {
        let said: Vec<String> = trace
            .phase_order()
            .into_iter()
            .filter_map(|p| phases.get(&p).map(|v| (p, *v)))
            .map(|(p, v)| {
                phase_json.insert(p.clone(), json!(round(v)));
                format!("{p} {}", ms(v))
            })
            .collect();
        text.push_str(&format!("\nphases: {}\n", said.join(", ")));
    }
    let gpu = trace.gpu_of(pos);
    let gpu_ms: f64 = gpu.iter().map(|g| g.dur_ns as f64 / 1e6).sum();
    if !gpu.is_empty() {
        let said: Vec<String> = gpu
            .iter()
            .map(|g| format!("{} {}", g.name, ms(g.dur_ns as f64 / 1e6)))
            .collect();
        text.push_str(&format!("gpu: {} ms ({})\n", ms(gpu_ms), said.join(", ")));
    }
    let notes = trace.notes_of(pos);
    for n in &notes {
        text.push_str(&format!(
            "note: {} {}{}\n",
            n.kind,
            n.detail,
            n.ms.map(|m| format!(" {} ms", ms(m))).unwrap_or_default()
        ));
    }
    if min_ns > 0 {
        text.push_str(&format!(
            "\n  scopes under {} ms left out (--min-ms); a name run more than once under one parent is one row\n",
            ms(args.min_ms)
        ));
    }

    let doc = json!({
        "trace": about_json(trace),
        "frame": span.frame,
        "ms": round(frame_ms),
        "median_ms": round(normal.median_ms),
        "normal_frames": normal.positions.len(),
        "min_ms": args.min_ms,
        "hitch": hitch.map(|h| json!({
            "cause": h.cause, "judged_ms": h.judged_ms, "median_ms": h.median_ms, "game_time": h.game_time
        })),
        "threads": json_threads,
        "phases": phase_json,
        "gpu": {
            "ms": round(gpu_ms),
            "passes": gpu.iter().map(|g| json!({"pass": g.name, "ms": round(g.dur_ns as f64 / 1e6)})).collect::<Vec<_>>(),
        },
        "notes": notes.iter().map(|n| json!({"kind": n.kind, "detail": n.detail, "ms": n.ms, "depth": n.depth})).collect::<Vec<_>>(),
    });
    Ok((text, doc))
}
