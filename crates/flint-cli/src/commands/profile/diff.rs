//! `flint profile diff`: two traces' scopes, per frame, side by side.
//!
//! Each scope's time a frame (a frame it didn't run in counting 0) over
//! each trace's frames but the first and last, its mean and p90, and the
//! deltas. Two runs of the same build differ by more than nothing: a
//! delta inside twice the standard error of the two means (the frames'
//! own spread, [`NOISE_SE`]) is marked `~` and, without `--all`, not
//! listed, since run-to-run spread can make it.

use super::read::Trace;
use super::stats::{self, ms, round, signed};
use super::{about_json, DiffArgs};
use crate::commands::frame::table::Table;
use crate::output::{self, Answer};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};

/// A delta under this many standard errors of the difference of means is
/// within noise.
pub(crate) const NOISE_SE: f64 = 2.0;

pub fn run(args: DiffArgs) -> Result<()> {
    let a = Trace::open(&args.a)?;
    let b = Trace::open(&args.b)?;
    let (text, doc) = diff(&a, &b, &args)?;
    output::emit(&Answer::new(text, doc))
}

/// One trace's frames, as a diff reads them.
struct Side {
    n: usize,
    frame_ms: Vec<f64>,
    keys: HashMap<String, Vec<f64>>,
}

fn side(trace: &Trace, thread: Option<&str>) -> Result<Side> {
    let threads = trace.threads_named(thread)?;
    let per = stats::per_frame(trace, &threads, false);
    let len = trace.frames.len();
    let positions: Vec<usize> = (0..len)
        .filter(|&i| len <= 2 || (i != 0 && i + 1 != len))
        .collect();
    let keys = stats::values(&per, &positions, |k| k.incl_ms())
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    Ok(Side {
        n: positions.len(),
        frame_ms: positions.iter().map(|&p| trace.frames[p].ms()).collect(),
        keys,
    })
}

/// Mean, p90 and standard deviation of `n` values (`present`, rest 0).
fn summary(present: &[f64], n: usize) -> (f64, f64, f64) {
    let (mean, sd) = stats::mean_sd(present, n);
    (mean, stats::quantile(present, n, 0.9), sd)
}

struct Delta {
    key: String,
    a: (f64, f64),
    b: (f64, f64),
    se: f64,
}

impl Delta {
    fn mean(&self) -> f64 {
        self.b.0 - self.a.0
    }
    fn p90(&self) -> f64 {
        self.b.1 - self.a.1
    }
    fn noise(&self) -> bool {
        self.mean().abs() < NOISE_SE * self.se
    }
    fn json(&self) -> Value {
        json!({
            "scope": self.key,
            "a": { "mean_ms": round(self.a.0), "p90_ms": round(self.a.1) },
            "b": { "mean_ms": round(self.b.0), "p90_ms": round(self.b.1) },
            "delta_mean_ms": round(self.mean()),
            "delta_p90_ms": round(self.p90()),
            "noise_ms": round(NOISE_SE * self.se),
            "within_noise": self.noise(),
        })
    }
}

fn delta(key: &str, a: (&[f64], usize), b: (&[f64], usize)) -> Delta {
    let (am, ap, asd) = summary(a.0, a.1);
    let (bm, bp, bsd) = summary(b.0, b.1);
    let se = (asd * asd / a.1.max(1) as f64 + bsd * bsd / b.1.max(1) as f64).sqrt();
    Delta {
        key: key.to_string(),
        a: (am, ap),
        b: (bm, bp),
        se,
    }
}

fn diff(a: &Trace, b: &Trace, args: &DiffArgs) -> Result<(String, Value)> {
    let sa = side(a, args.thread.as_deref())?;
    let sb = side(b, args.thread.as_deref())?;
    let frame = delta("frame", (&sa.frame_ms, sa.n), (&sb.frame_ms, sb.n));

    let mut text = format!("a: {}\nb: {}\n", a.path, b.path);
    let mut warnings = Vec::new();
    for key in ["scene", "build", "adapter", "backend"] {
        let (x, y) = (a.header_str(key), b.header_str(key));
        if let (Some(x), Some(y)) = (x, y) {
            if x != y {
                warnings.push(format!("the {key}s differ: {x} against {y}"));
            }
        }
    }
    for w in &warnings {
        text.push_str(&format!("  note: {w}\n"));
    }
    text.push_str(&format!(
        "\nframe: mean {} -> {} ms ({}), p90 {} -> {} ({}), over {} and {} frames\n",
        ms(frame.a.0),
        ms(frame.b.0),
        signed(frame.mean()),
        ms(frame.a.1),
        ms(frame.b.1),
        signed(frame.p90()),
        sa.n,
        sb.n
    ));
    if frame.noise() {
        text.push_str(&format!(
            "  the frame time's delta is inside its own spread (+-{} ms, {NOISE_SE} standard errors): \
             run both again before believing it\n",
            ms(NOISE_SE * frame.se)
        ));
    }

    let keys: BTreeSet<&String> = sa.keys.keys().chain(sb.keys.keys()).collect();
    let empty = Vec::new();
    let mut rows: Vec<Delta> = keys
        .into_iter()
        .map(|k| {
            delta(
                k,
                (sa.keys.get(k).unwrap_or(&empty), sa.n),
                (sb.keys.get(k).unwrap_or(&empty), sb.n),
            )
        })
        .collect();
    rows.sort_by(|x, y| {
        y.mean()
            .abs()
            .total_cmp(&x.mean().abs())
            .then(x.key.cmp(&y.key))
    });
    let noisy = rows.iter().filter(|d| d.noise()).count();
    let shown: Vec<&Delta> = rows
        .iter()
        .filter(|d| args.all || !d.noise())
        .take(args.limit)
        .collect();

    if shown.is_empty() {
        text.push_str("\nno scope's delta is outside the frames' spread\n");
    } else {
        let mut table = Table::new(&[
            "scope",
            ">a mean",
            ">b mean",
            ">delta",
            ">a p90",
            ">b p90",
            ">delta p90",
            "",
        ]);
        for d in &shown {
            let only = match (sa.keys.contains_key(&d.key), sb.keys.contains_key(&d.key)) {
                (false, true) => "only in b",
                (true, false) => "only in a",
                _ if d.noise() => "~",
                _ => "",
            };
            table.row(vec![
                d.key.clone(),
                ms(d.a.0),
                ms(d.b.0),
                signed(d.mean()),
                ms(d.a.1),
                ms(d.b.1),
                signed(d.p90()),
                only.to_string(),
            ]);
        }
        text.push_str("\nms a frame, by the size of the mean's delta:\n");
        text.push_str(&table.render("  "));
    }
    if noisy > 0 {
        text.push_str(&format!(
            "\n  {noisy} scope{} moved less than {NOISE_SE} standard errors of their frames' spread{}\n",
            if noisy == 1 { "" } else { "s" },
            if args.all {
                " (marked ~)"
            } else {
                ": --all lists them, marked ~"
            }
        ));
    }

    let doc = json!({
        "a": about_json(a),
        "b": about_json(b),
        "notes": warnings,
        "frame": frame.json(),
        "frames": { "a": sa.n, "b": sb.n },
        "scopes": shown.iter().map(|d| {
            let mut v = d.json();
            v["only_in"] = match (sa.keys.contains_key(&d.key), sb.keys.contains_key(&d.key)) {
                (false, true) => json!("b"),
                (true, false) => json!("a"),
                _ => Value::Null,
            };
            v
        }).collect::<Vec<_>>(),
        "within_noise": noisy,
        "noise_rule": format!("a delta under {NOISE_SE} standard errors of the difference of the two means"),
    });
    Ok((text, doc))
}
