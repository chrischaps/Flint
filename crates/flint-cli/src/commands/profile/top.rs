//! `flint profile top`: every scope's time over the trace (or `--frames`),
//! by total, self, worst call or calls.

use super::stats::{self, ms, round};
use super::{about, about_json, parse_range, TopArgs, TopSort};
use crate::commands::frame::table::Table;
use crate::commands::trace::Trace;
use crate::output::{self, Answer};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn run(args: TopArgs) -> Result<()> {
    let trace = Trace::open(&args.trace)?;
    let (text, doc) = top(&trace, &args)?;
    output::emit(&Answer::new(text, doc))
}

#[derive(Default)]
struct Row {
    incl_ns: u64,
    self_ns: u64,
    calls: u64,
    worst_ns: u64,
    worst_frame: u32,
    frames: usize,
}

fn top(trace: &Trace, args: &TopArgs) -> Result<(String, Value)> {
    let threads = trace.threads_named(args.thread.as_deref())?;
    let (from, to) = match &args.frames {
        Some(r) => parse_range(r)?,
        None => (0, u32::MAX),
    };
    let positions: Vec<usize> = trace
        .covered()
        .filter(|&i| (from..=to).contains(&trace.frames[i].frame))
        .collect();
    if positions.is_empty() {
        let covered = trace.covered_frames();
        let held = match &trace.coverage {
            Some(c) => c.line(&trace.frames),
            None => format!(
                "it holds frames {}..{}",
                covered[0].frame,
                covered[covered.len() - 1].frame
            ),
        };
        bail!(
            "no frame of {} with scopes is in {}: {held}",
            trace.path,
            args.frames.as_deref().unwrap_or(""),
        );
    }
    let per = stats::per_frame(trace, &threads, args.by_detail);
    let mut rows: HashMap<&str, Row> = HashMap::new();
    for &p in &positions {
        for (k, v) in &per[p] {
            let r = rows.entry(k.as_str()).or_default();
            r.incl_ns += v.incl_ns;
            r.self_ns += v.self_ns;
            r.calls += v.calls as u64;
            r.frames += 1;
            if v.worst_ns > r.worst_ns {
                r.worst_ns = v.worst_ns;
                r.worst_frame = trace.frames[p].frame;
            }
        }
    }
    let mut rows: Vec<(&str, Row)> = rows.into_iter().collect();
    rows.sort_by(|(an, a), (bn, b)| {
        let (x, y) = match args.sort {
            TopSort::Total => (a.incl_ns, b.incl_ns),
            TopSort::SelfTime => (a.self_ns, b.self_ns),
            TopSort::Worst => (a.worst_ns, b.worst_ns),
            TopSort::Calls => (a.calls, b.calls),
        };
        y.cmp(&x).then(an.cmp(bn))
    });
    let listed = rows.len();
    rows.truncate(args.limit);

    let n = positions.len();
    let frame_ms: Vec<f64> = positions.iter().map(|&p| trace.frames[p].ms()).collect();
    let (mean, _) = stats::mean_sd(&frame_ms, n);
    let (p50, p90) = (
        stats::quantile(&frame_ms, n, 0.5),
        stats::quantile(&frame_ms, n, 0.9),
    );
    let max = frame_ms.iter().copied().fold(0.0, f64::max);
    let (first, last) = (
        trace.frames[positions[0]].frame,
        trace.frames[positions[n - 1]].frame,
    );

    let mut text = about(trace);
    text.push_str(&format!(
        "\n{n} frames ({first}..{last}): mean {} ms, p50 {}, p90 {}, max {}\n",
        ms(mean),
        ms(p50),
        ms(p90),
        ms(max)
    ));
    let threads_said: Vec<&str> = threads.iter().map(|t| t.name.as_str()).collect();
    text.push_str(&format!(
        "{listed} scopes on {}; by {}{}\n\n",
        threads_said.join(", "),
        match args.sort {
            TopSort::Total => "total",
            TopSort::SelfTime => "self time",
            TopSort::Worst => "worst call",
            TopSort::Calls => "calls",
        },
        if listed > rows.len() {
            format!(", the first {}", rows.len())
        } else {
            String::new()
        }
    ));
    let mut table = Table::new(&[
        "scope",
        ">calls",
        ">total ms",
        ">self ms",
        ">ms/frame",
        ">worst ms",
        ">in frame",
    ]);
    let mut json_rows = Vec::new();
    for (name, r) in &rows {
        let per_frame = r.incl_ns as f64 / 1e6 / n as f64;
        table.row(vec![
            name.to_string(),
            r.calls.to_string(),
            ms(r.incl_ns as f64 / 1e6),
            ms(r.self_ns as f64 / 1e6),
            ms(per_frame),
            ms(r.worst_ns as f64 / 1e6),
            r.worst_frame.to_string(),
        ]);
        json_rows.push(json!({
            "scope": name,
            "calls": r.calls,
            "total_ms": round(r.incl_ns as f64 / 1e6),
            "self_ms": round(r.self_ns as f64 / 1e6),
            "mean_ms_per_frame": round(per_frame),
            "worst_ms": round(r.worst_ns as f64 / 1e6),
            "worst_frame": r.worst_frame,
            "frames_ran": r.frames,
        }));
    }
    text.push_str(&table.render("  "));
    text.push_str(if trace.coverage.is_some() {
        "\n  ms/frame is the total over the frames in range the scopes cover; a scope inside itself counts once\n"
    } else {
        "\n  ms/frame is the total over every frame in range; a scope inside itself counts once\n"
    });

    let doc = json!({
        "trace": about_json(trace),
        "range": { "first": first, "last": last, "frames": n },
        "frame_ms": {
            "mean": round(mean), "p50": round(p50), "p90": round(p90), "max": round(max)
        },
        "threads": threads_said,
        "sort": match args.sort {
            TopSort::Total => "total",
            TopSort::SelfTime => "self",
            TopSort::Worst => "worst",
            TopSort::Calls => "calls",
        },
        "scopes_listed": rows.len(),
        "scopes_in_range": listed,
        "scopes": json_rows,
    });
    Ok((text, doc))
}
