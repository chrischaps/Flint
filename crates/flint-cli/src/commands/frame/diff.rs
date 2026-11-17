//! `flint frame diff <a> <b>`: what changed between two captures.
//!
//! Passes are paired by name and which of that name they are (`Bloom (2)`),
//! entities by name, never by id (ids move between runs, ADR 0126), targets
//! by id, and images by pass, target, step and layer. The diff reads both
//! folders only, so two captures from two machines or two commits compare.

use super::image_out::{exposure, viewable};
use super::read::{self, Capture, Texels};
use super::table::Table;
use super::{bytes, bytes_delta, passes, DiffArgs, PassAt};
use crate::output::{self, Answer};
use anyhow::{Context, Result};
use flint_render::frame_capture::ImageDescription;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

pub fn run(args: DiffArgs) -> Result<()> {
    let a = Capture::open(&args.a)?;
    let b = Capture::open(&args.b)?;
    if let Some(dir) = &args.out {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let doc = diff(&a, &b, &args)?;
    let text = text(&Capture::shown(&args.a), &Capture::shown(&args.b), &doc);
    output::emit(&Answer::new(text, doc))
}

fn change<T: Into<f64> + Copy>(a: T, b: T) -> Value {
    json!({ "a": a.into(), "b": b.into(), "delta": b.into() - a.into() })
}

fn diff(a: &Capture, b: &Capture, args: &DiffArgs) -> Result<Value> {
    let (da, db) = (&a.description, &b.description);
    let (pa, pb) = (passes(da), passes(db));
    let by_key = |ps: &[PassAt<'_>]| -> Vec<String> { ps.iter().map(PassAt::key).collect() };
    let (ka, kb) = (by_key(&pa), by_key(&pb));
    let (sa, sb): (BTreeSet<&String>, BTreeSet<&String>) =
        (ka.iter().collect(), kb.iter().collect());
    let added: Vec<&String> = kb.iter().filter(|k| !sa.contains(k)).collect();
    let removed: Vec<&String> = ka.iter().filter(|k| !sb.contains(k)).collect();
    // Moved: the passes both have, in a's order, that aren't on the longest
    // run they share in order
    let common_a: Vec<&String> = ka.iter().filter(|k| sb.contains(k)).collect();
    let common_b: Vec<&String> = kb.iter().filter(|k| sa.contains(k)).collect();
    let kept = lcs(&common_a, &common_b);
    let moved: Vec<&&String> = common_a.iter().filter(|k| !kept.contains(*k)).collect();

    let find = |ps: &'_ [PassAt<'_>], key: &str| ps.iter().position(|p| p.key() == key);
    let mut ran_changed = Vec::new();
    let mut changed = Vec::new();
    for key in &common_a {
        let (x, y) = (&pa[find(&pa, key).unwrap()], &pb[find(&pb, key).unwrap()]);
        if x.pass.ran != y.pass.ran {
            ran_changed.push(json!({
                "pass": key,
                "a": if x.pass.ran { "ran".into() } else { format!("skipped: {}", x.pass.skipped.as_deref().unwrap_or("")) },
                "b": if y.pass.ran { "ran".into() } else { format!("skipped: {}", y.pass.skipped.as_deref().unwrap_or("")) },
            }));
        }
        let tris = |p: &PassAt| p.pass.draws.iter().map(|d| d.triangles as u64).sum::<u64>() as f64;
        let draws_changed = x.pass.draws.len() != y.pass.draws.len() || tris(x) != tris(y);
        let gpu = match (x.pass.gpu_ms, y.pass.gpu_ms) {
            (Some(g), Some(h)) => Some(change(g, h)),
            _ => None,
        };
        if draws_changed || gpu.is_some() {
            changed.push(json!({
                "pass": key,
                "gpu_ms": gpu,
                "draws": change(x.pass.draws.len() as f64, y.pass.draws.len() as f64),
                "triangles": change(tris(x), tris(y)),
            }));
        }
    }

    // Each entity's draws, pass by pass
    let mut appeared = Vec::new();
    let mut vanished = Vec::new();
    let mut entity_changed = Vec::new();
    let all_keys: Vec<&String> = ka.iter().chain(added.iter().copied()).collect();
    for key in all_keys {
        let x = find(&pa, key).map(|i| &pa[i]);
        let y = find(&pb, key).map(|i| &pb[i]);
        let (ex, ey) = (
            x.map(entity_draws).unwrap_or_default(),
            y.map(entity_draws).unwrap_or_default(),
        );
        for (entity, dx) in &ex {
            match ey.get(entity) {
                None => vanished.push(json!({ "pass": key, "entity": entity, "draws": dx.count, "pipelines": dx.pipelines })),
                Some(dy) if dy != dx => entity_changed.push(json!({
                    "pass": key,
                    "entity": entity,
                    "a": { "draws": dx.count, "pipelines": dx.pipelines, "triangles": dx.triangles },
                    "b": { "draws": dy.count, "pipelines": dy.pipelines, "triangles": dy.triangles },
                })),
                Some(_) => {}
            }
        }
        for (entity, dy) in &ey {
            if !ex.contains_key(entity) {
                appeared.push(json!({ "pass": key, "entity": entity, "draws": dy.count, "pipelines": dy.pipelines }));
            }
        }
    }

    // Targets
    let ta: BTreeMap<&str, _> = da.targets.iter().map(|t| (t.id.as_str(), t)).collect();
    let tb: BTreeMap<&str, _> = db.targets.iter().map(|t| (t.id.as_str(), t)).collect();
    let targets_added: Vec<Value> = tb
        .iter()
        .filter(|(k, _)| !ta.contains_key(*k))
        .map(|(k, t)| json!({ "id": k, "bytes": t.bytes, "format": t.format, "size": t.size }))
        .collect();
    let targets_removed: Vec<Value> = ta
        .iter()
        .filter(|(k, _)| !tb.contains_key(*k))
        .map(|(k, t)| json!({ "id": k, "bytes": t.bytes, "format": t.format, "size": t.size }))
        .collect();
    let targets_changed: Vec<Value> = ta
        .iter()
        .filter_map(|(k, x)| {
            let y = tb.get(k)?;
            (x != y).then(|| {
                json!({
                    "id": k,
                    "bytes": change(x.bytes as f64, y.bytes as f64),
                    "a": { "format": x.format, "size": x.size, "layers": x.layers, "samples": x.samples },
                    "b": { "format": y.format, "size": y.size, "layers": y.layers, "samples": y.samples },
                })
            })
        })
        .collect();

    let images = image_diffs(a, b, &pa, &pb, args)?;

    let (fa, fb) = (&da.frame, &db.frame);
    Ok(json!({
        "a": Capture::shown(&args.a),
        "b": Capture::shown(&args.b),
        "scene": {
            "a": da.scene.as_ref().map(|s| &s.path),
            "b": db.scene.as_ref().map(|s| &s.path),
            "same_file": match (&da.scene, &db.scene) {
                (Some(x), Some(y)) => Value::Bool(x.path == y.path),
                _ => Value::Null,
            },
            "same_text": match (&da.scene, &db.scene) {
                (Some(x), Some(y)) if x.sha256.is_some() => Value::Bool(x.sha256 == y.sha256),
                _ => Value::Null,
            },
        },
        "frame": {
            "size": { "a": fa.size, "b": fb.size },
            "gpu_frame_ms": match (fa.gpu_frame_ms, fb.gpu_frame_ms) {
                (Some(x), Some(y)) => change(x, y),
                _ => Value::Null,
            },
            "draw_calls": change(da.stats.draw_calls, db.stats.draw_calls),
            "triangles": change(da.stats.triangles, db.stats.triangles),
            "shadow_draw_calls": change(da.stats.shadow_draw_calls, db.stats.shadow_draw_calls),
            "culled_draws": change(da.stats.culled_draws, db.stats.culled_draws),
            "target_bytes": change(da.memory.target_bytes as f64, db.memory.target_bytes as f64),
        },
        "passes": {
            "added": added,
            "removed": removed,
            "moved": moved,
            "ran_changed": ran_changed,
            "changed": changed,
        },
        "draws": {
            "appeared": appeared,
            "vanished": vanished,
            "changed": entity_changed,
        },
        "targets": {
            "added": targets_added,
            "removed": targets_removed,
            "changed": targets_changed,
        },
        "images": images,
    }))
}

/// One entity's draws in one pass, compared as a whole.
#[derive(Default, PartialEq)]
struct EntityDraws {
    count: usize,
    triangles: u64,
    /// Each pipeline it draws through, sorted, repeats kept.
    pipelines: Vec<String>,
}

/// A pass's draws by entity name; a draw with no entity (grass, a sprite
/// batch) goes under its kind, in brackets.
fn entity_draws(p: &PassAt) -> BTreeMap<String, EntityDraws> {
    let mut out: BTreeMap<String, EntityDraws> = BTreeMap::new();
    for d in &p.pass.draws {
        let name = d.entity.clone().unwrap_or_else(|| format!("({})", d.kind));
        let e = out.entry(name).or_default();
        e.count += 1;
        e.triangles += d.triangles as u64;
        e.pipelines.push(d.pipeline.clone());
    }
    for e in out.values_mut() {
        e.pipelines.sort();
    }
    out
}

/// The longest run two lists share in order.
fn lcs<'a>(a: &[&'a String], b: &[&'a String]) -> BTreeSet<&'a String> {
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] {
                t[i + 1][j + 1] + 1
            } else {
                t[i + 1][j].max(t[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, BTreeSet::new());
    while i < n && j < m {
        if a[i] == b[j] {
            out.insert(a[i]);
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// What identifies an image across captures.
fn image_key(pass: &str, i: &ImageDescription) -> String {
    let mut key = format!("{pass} / {}", i.target);
    if let Some(step) = &i.step {
        let _ = write!(key, " / {step}");
    }
    if let Some(layer) = i.layer {
        let _ = write!(key, " / layer {layer}");
    }
    key
}

fn keyed_images<'a>(ps: &'a [PassAt<'a>]) -> Vec<(String, &'a ImageDescription)> {
    let mut out = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for p in ps {
        for i in &p.pass.images {
            let mut key = image_key(&p.key(), i);
            let n = seen.entry(key.clone()).or_default();
            if *n > 0 {
                let _ = write!(key, " ({})", *n + 1);
            }
            *n += 1;
            out.push((key, i));
        }
    }
    out
}

fn image_diffs(
    a: &Capture,
    b: &Capture,
    pa: &[PassAt],
    pb: &[PassAt],
    args: &DiffArgs,
) -> Result<Vec<Value>> {
    let ia = keyed_images(pa);
    let ib = keyed_images(pb);
    let lookup_b: BTreeMap<&str, &ImageDescription> =
        ib.iter().map(|(k, i)| (k.as_str(), *i)).collect();
    let keys_a: BTreeSet<&str> = ia.iter().map(|(k, _)| k.as_str()).collect();
    let mut out = Vec::new();
    for (key, x) in &ia {
        let Some(y) = lookup_b.get(key.as_str()) else {
            out.push(json!({ "image": key, "status": "only_a", "a": x.file }));
            continue;
        };
        out.push(compare(a, b, key, x, y, args));
    }
    for (key, y) in &ib {
        if !keys_a.contains(key.as_str()) {
            out.push(json!({ "image": key, "status": "only_b", "b": y.file }));
        }
    }
    Ok(out)
}

fn compare(
    a: &Capture,
    b: &Capture,
    key: &str,
    x: &ImageDescription,
    y: &ImageDescription,
    args: &DiffArgs,
) -> Value {
    let base = json!({ "image": key, "a": x.file, "b": y.file });
    let with = |mut v: Value, extra: Value| {
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        v
    };
    let (fa, fb) = (a.file(x), b.file(y));
    if x.stored == "raw" || y.stored == "raw" {
        let same = std::fs::read(&fa).ok() == std::fs::read(&fb).ok();
        return with(
            base,
            json!({ "status": if same { "same" } else { "differs" }, "note": "raw texels: compared as bytes" }),
        );
    }
    let read = |path: &Path, i: &ImageDescription| read::texels(path, i);
    let (ta, tb) = match (read(&fa, x), read(&fb, y)) {
        (Ok(ta), Ok(tb)) => (ta, tb),
        (Err(e), _) | (_, Err(e)) => {
            return with(
                base,
                json!({ "status": "unreadable", "error": format!("{e:#}") }),
            )
        }
    };
    if (ta.width, ta.height, ta.channels) != (tb.width, tb.height, tb.channels) {
        return with(
            base,
            json!({ "status": "size_changed", "a_size": [ta.width, ta.height], "b_size": [tb.width, tb.height] }),
        );
    }
    let (differing, max_delta, deltas) = pixel_delta(&ta, &tb, args.tolerance);
    let pixels = ta.pixels();
    let mut extra = json!({
        "status": if differing == 0 { "same" } else { "differs" },
        "differing": differing,
        "pixels": pixels,
        "max_delta": max_delta,
        "format": y.format,
    });
    if differing > 0 {
        if let Some(dir) = &args.out {
            let stem = key
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            let stem: String = stem
                .split('_')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("_");
            let path = dir.join(format!("{stem}.diff.png"));
            let picture = diff_picture(&tb, y, &deltas, max_delta, exposure(&b.description));
            match picture.save(&path) {
                Ok(()) => extra["diff"] = json!(path.display().to_string().replace('\\', "/")),
                Err(e) => extra["diff_error"] = json!(e.to_string()),
            }
        }
    }
    with(base, extra)
}

/// How many pixels differ past `tolerance` (in each image's own units, an
/// 8-bit step normalized to 1/255), the largest difference, and each
/// pixel's.
pub(crate) fn pixel_delta(a: &Texels, b: &Texels, tolerance: f32) -> (usize, f32, Vec<f32>) {
    let scale = a.scale();
    let mut deltas = vec![0.0f32; a.pixels()];
    let (mut differing, mut max) = (0, 0.0f32);
    for (i, slot) in deltas.iter_mut().enumerate() {
        let d = a
            .texel(i)
            .iter()
            .zip(b.texel(i))
            .map(|(x, y)| {
                // NaN against NaN is the same texel; NaN against a number isn't
                if x.is_nan() && y.is_nan() {
                    0.0
                } else if x.is_nan() || y.is_nan() {
                    f32::INFINITY
                } else {
                    (x - y).abs() / scale
                }
            })
            .fold(0.0f32, f32::max);
        if d > tolerance {
            differing += 1;
            *slot = d;
            max = max.max(d);
        }
    }
    (
        differing,
        if max.is_finite() { max } else { f32::MAX },
        deltas,
    )
}

/// The after image dimmed to a cool grey, each differing pixel lit from
/// amber to white by how far it moved: what changed stands out, and where.
fn diff_picture(
    b: &Texels,
    image: &ImageDescription,
    deltas: &[f32],
    max: f32,
    exposure: f32,
) -> image::RgbaImage {
    let (shown, _) = viewable(b, image, exposure);
    let mut out = shown;
    let max = if max.is_finite() && max > 0.0 {
        max
    } else {
        1.0
    };
    for (i, px) in out.pixels_mut().enumerate() {
        let [r, g, bl, _] = px.0;
        let luma = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * bl as f32) / 255.0;
        let d = deltas[i];
        let rgb = if d > 0.0 {
            // At least a third lit, so a one-step change is still seen
            let t = (d / max).clamp(0.0, 1.0).sqrt() * 0.67 + 0.33;
            [1.0, 0.55 + 0.45 * t, 0.12 + 0.88 * t * t].map(|c| c * (0.55 + 0.45 * t))
        } else {
            let base = 0.06 + 0.22 * luma;
            [base * 0.92, base * 0.97, base * 1.08]
        };
        px.0 = [
            (rgb[0].clamp(0.0, 1.0) * 255.0) as u8,
            (rgb[1].clamp(0.0, 1.0) * 255.0) as u8,
            (rgb[2].clamp(0.0, 1.0) * 255.0) as u8,
            255,
        ];
    }
    out
}

// ── Text ──

fn delta_text(v: &Value, digits: usize) -> String {
    let (a, b, mut d) = (
        v["a"].as_f64().unwrap_or(0.0),
        v["b"].as_f64().unwrap_or(0.0),
        v["delta"].as_f64().unwrap_or(0.0),
    );
    // Below the last digit shown, a change is none (no "-0.000")
    if d.abs() < 0.5 * 10f64.powi(-(digits as i32)) {
        d = 0.0;
    }
    format!(
        "{a:.digits$} -> {b:.digits$} ({}{d:.digits$})",
        if d >= 0.0 { "+" } else { "" }
    )
}

fn list(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| x.as_str().map_or(x.to_string(), String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// An entity's draws in a pass, the kinds counted (#1208): `91 draws:
/// skinned shadow` when every draw is one kind, `3 draws (2 pbr, 1 unlit)`
/// otherwise, the most-drawn kind first. A skinned body of 91 parts used
/// to print its kind 91 times.
fn draws_text(draws: &Value, pipelines: &Value) -> String {
    let n = draws.as_u64().unwrap_or(0);
    let noun = if n == 1 { "draw" } else { "draws" };
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for kind in list(pipelines) {
        match kinds.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, count)) => *count += 1,
            None => kinds.push((kind, 1)),
        }
    }
    kinds.sort_by(|(ka, a), (kb, b)| b.cmp(a).then_with(|| ka.cmp(kb)));
    match kinds.as_slice() {
        [] => format!("{n} {noun}"),
        [(kind, _)] => format!("{n} {noun}: {kind}"),
        _ => {
            let counted: Vec<String> = kinds.iter().map(|(k, c)| format!("{c} {k}")).collect();
            format!("{n} {noun} ({})", counted.join(", "))
        }
    }
}

fn text(a: &str, b: &str, doc: &Value) -> String {
    let mut out = format!("a  {a}\nb  {b}\n");
    let scene = &doc["scene"];
    if scene["same_file"] == Value::Bool(false) {
        let _ = writeln!(
            out,
            "   (scene files: {} and {})",
            scene["a"].as_str().unwrap_or("?"),
            scene["b"].as_str().unwrap_or("?")
        );
    } else if scene["same_text"] == Value::Bool(false) {
        out.push_str("   (the scene file changed between them)\n");
    }
    let f = &doc["frame"];
    out.push_str("\nFrame\n");
    if f["size"]["a"] != f["size"]["b"] {
        let _ = writeln!(
            out,
            "  size         {} -> {}",
            f["size"]["a"], f["size"]["b"]
        );
    }
    if f["gpu_frame_ms"].is_object() {
        let _ = writeln!(out, "  GPU ms       {}", delta_text(&f["gpu_frame_ms"], 3));
    }
    for (label, key) in [
        ("draws", "draw_calls"),
        ("triangles", "triangles"),
        ("shadow draws", "shadow_draw_calls"),
        ("culled", "culled_draws"),
    ] {
        if f[key]["delta"].as_f64() != Some(0.0) {
            let _ = writeln!(out, "  {label:<12} {}", delta_text(&f[key], 0));
        }
    }
    let tb = &f["target_bytes"];
    if tb["delta"].as_f64() != Some(0.0) {
        let _ = writeln!(
            out,
            "  memory       {} -> {} ({})",
            bytes(tb["a"].as_f64().unwrap_or(0.0) as u64),
            bytes(tb["b"].as_f64().unwrap_or(0.0) as u64),
            bytes_delta(tb["delta"].as_f64().unwrap_or(0.0) as i64)
        );
    }

    let p = &doc["passes"];
    out.push_str("\nPasses\n");
    let mut any = false;
    for (label, key) in [
        ("added", "added"),
        ("removed", "removed"),
        ("moved", "moved"),
    ] {
        let names = list(&p[key]);
        if !names.is_empty() {
            any = true;
            let _ = writeln!(out, "  {label:<8} {}", names.join(", "));
        }
    }
    for r in p["ran_changed"].as_array().into_iter().flatten() {
        any = true;
        let _ = writeln!(
            out,
            "  {:<8} {}: {} -> {}",
            "switched",
            r["pass"].as_str().unwrap_or(""),
            r["a"].as_str().unwrap_or(""),
            r["b"].as_str().unwrap_or("")
        );
    }
    let changed = p["changed"].as_array().cloned().unwrap_or_default();
    let mut t = Table::new(&["pass", ">gpu ms", ">delta", ">draws", ">triangles"]);
    for c in &changed {
        let gpu = &c["gpu_ms"];
        let draws = &c["draws"];
        let tris = &c["triangles"];
        let moved = draws["delta"].as_f64() != Some(0.0) || tris["delta"].as_f64() != Some(0.0);
        let timed_change = gpu["delta"].as_f64().is_some_and(|d| d.abs() >= 0.0005);
        if !moved && !timed_change {
            continue;
        }
        let pair = |v: &Value| {
            if v["a"].as_f64() == Some(0.0) && v["b"].as_f64() == Some(0.0) {
                String::new()
            } else if v["delta"].as_f64() == Some(0.0) {
                format!("{}", v["a"].as_f64().unwrap_or(0.0))
            } else {
                format!(
                    "{} -> {}",
                    v["a"].as_f64().unwrap_or(0.0),
                    v["b"].as_f64().unwrap_or(0.0)
                )
            }
        };
        t.row(vec![
            c["pass"].as_str().unwrap_or("").to_string(),
            if gpu.is_object() {
                format!("{:.3}", gpu["b"].as_f64().unwrap_or(0.0))
            } else {
                "-".into()
            },
            if gpu.is_object() {
                let d = gpu["delta"].as_f64().unwrap_or(0.0);
                format!("{}{d:.3}", if d >= 0.0 { "+" } else { "" })
            } else {
                String::new()
            },
            pair(draws),
            pair(tris),
        ]);
    }
    if !t.is_empty() {
        any = true;
        out.push_str(&t.render("  "));
    }
    if !any {
        out.push_str("  the same passes in the same order\n");
    }

    let d = &doc["draws"];
    out.push_str("\nDraws by entity\n");
    let mut any = false;
    for (label, key) in [("removed", "vanished"), ("added", "appeared")] {
        for r in d[key].as_array().into_iter().flatten() {
            any = true;
            let _ = writeln!(
                out,
                "  {label:<8} {} in {}: {}",
                r["entity"].as_str().unwrap_or(""),
                r["pass"].as_str().unwrap_or(""),
                draws_text(&r["draws"], &r["pipelines"])
            );
        }
    }
    for r in d["changed"].as_array().into_iter().flatten() {
        any = true;
        let _ = writeln!(
            out,
            "  changed  {} in {}: {} -> {}",
            r["entity"].as_str().unwrap_or(""),
            r["pass"].as_str().unwrap_or(""),
            draws_text(&r["a"]["draws"], &r["a"]["pipelines"]),
            draws_text(&r["b"]["draws"], &r["b"]["pipelines"])
        );
    }
    if !any {
        out.push_str("  every entity draws as it did\n");
    }

    let tg = &doc["targets"];
    let mut lines = Vec::new();
    for r in tg["added"].as_array().into_iter().flatten() {
        lines.push(format!(
            "  added    {} ({}, +{})",
            r["id"].as_str().unwrap_or(""),
            r["format"].as_str().unwrap_or(""),
            bytes(r["bytes"].as_u64().unwrap_or(0))
        ));
    }
    for r in tg["removed"].as_array().into_iter().flatten() {
        lines.push(format!(
            "  removed  {} ({}, -{})",
            r["id"].as_str().unwrap_or(""),
            r["format"].as_str().unwrap_or(""),
            bytes(r["bytes"].as_u64().unwrap_or(0))
        ));
    }
    for r in tg["changed"].as_array().into_iter().flatten() {
        lines.push(format!(
            "  changed  {}: {} {}x{} -> {} {}x{} ({})",
            r["id"].as_str().unwrap_or(""),
            r["a"]["format"].as_str().unwrap_or(""),
            r["a"]["size"][0],
            r["a"]["size"][1],
            r["b"]["format"].as_str().unwrap_or(""),
            r["b"]["size"][0],
            r["b"]["size"][1],
            bytes_delta(r["bytes"]["delta"].as_f64().unwrap_or(0.0) as i64)
        ));
    }
    out.push_str("\nTargets\n");
    if lines.is_empty() {
        out.push_str("  the same targets at the same sizes\n");
    } else {
        for l in lines {
            out.push_str(&l);
            out.push('\n');
        }
    }

    let images = doc["images"].as_array().cloned().unwrap_or_default();
    let same = images.iter().filter(|i| i["status"] == "same").count();
    let _ = writeln!(out, "\nImages ({same} of {} the same)", images.len());
    let mut t = Table::new(&["image", ">differing", ">of", ">max", "diff"]);
    for i in &images {
        let status = i["status"].as_str().unwrap_or("");
        if status == "same" {
            continue;
        }
        let image = i["image"].as_str().unwrap_or("").to_string();
        match status {
            "differs" => {
                let n = i["differing"].as_u64().unwrap_or(0);
                let of = i["pixels"].as_u64().unwrap_or(0).max(1);
                t.row(vec![
                    image,
                    {
                        let share = 100.0 * n as f64 / of as f64;
                        if share < 0.1 {
                            format!("{n} (<0.1%)")
                        } else {
                            format!("{n} ({share:.1}%)")
                        }
                    },
                    of.to_string(),
                    i["max_delta"]
                        .as_f64()
                        .map_or(String::new(), |m| format!("{m:.4}")),
                    i["diff"].as_str().unwrap_or("").to_string(),
                ]);
            }
            other => t.row(vec![
                image,
                match other {
                    "only_a" => "only in a".into(),
                    "only_b" => "only in b".into(),
                    "size_changed" => format!("size {} -> {}", i["a_size"], i["b_size"]),
                    _ => i["error"].as_str().unwrap_or(other).to_string(),
                },
                String::new(),
                String::new(),
                String::new(),
            ]),
        }
    }
    if t.is_empty() {
        out.push_str("  every pass image is the same\n");
    } else {
        out.push_str(&t.render("  "));
        if images.iter().any(|i| i["status"] == "differs")
            && images.iter().all(|i| i["diff"].is_null())
        {
            out.push_str("  (--out <dir> writes a diff picture of each)\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_render::frame_capture::{DrawDescription, FrameDescription, PassDescription};

    fn draw(entity: &str, pipeline: &str) -> DrawDescription {
        DrawDescription {
            entity: Some(entity.into()),
            pipeline: pipeline.into(),
            kind: "Static".into(),
            triangles: 12,
            ..Default::default()
        }
    }

    fn frame(passes: Vec<PassDescription>) -> FrameDescription {
        FrameDescription {
            passes,
            ..Default::default()
        }
    }

    fn pass(name: &str, draws: Vec<DrawDescription>) -> PassDescription {
        PassDescription {
            name: name.into(),
            ran: true,
            draws,
            ..Default::default()
        }
    }

    fn capture(d: FrameDescription) -> Capture {
        Capture {
            dir: ".".into(),
            description: d,
        }
    }

    fn args() -> DiffArgs {
        DiffArgs {
            a: "a".into(),
            b: "b".into(),
            out: None,
            tolerance: 0.0,
        }
    }

    #[test]
    fn entities_pair_by_name_and_passes_by_order() {
        let a = capture(frame(vec![
            pass("Shadows", vec![draw("crate", "shadow")]),
            pass(
                "Scene",
                vec![
                    draw("crate", "pbr"),
                    draw("ground", "pbr"),
                    draw("lamp", "pbr"),
                ],
            ),
            pass("Bloom", vec![]),
            pass("Composite", vec![]),
        ]));
        let b = capture(frame(vec![
            pass(
                "Scene",
                vec![
                    draw("ground", "pbr"),
                    draw("lamp", "unlit"),
                    draw("rock", "pbr"),
                ],
            ),
            pass("Shadows", vec![]),
            pass("Composite", vec![]),
            pass("FXAA", vec![]),
        ]));
        let doc = diff(&a, &b, &args()).unwrap();
        assert_eq!(doc["passes"]["added"], json!(["FXAA"]));
        assert_eq!(doc["passes"]["removed"], json!(["Bloom"]));
        assert_eq!(doc["passes"]["moved"].as_array().unwrap().len(), 1);
        let vanished: Vec<(String, String)> = doc["draws"]["vanished"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v["pass"].as_str().unwrap().into(),
                    v["entity"].as_str().unwrap().into(),
                )
            })
            .collect();
        assert_eq!(
            vanished,
            [
                ("Shadows".into(), "crate".into()),
                ("Scene".into(), "crate".into())
            ]
        );
        assert_eq!(doc["draws"]["appeared"][0]["entity"], "rock");
        assert_eq!(doc["draws"]["changed"][0]["entity"], "lamp");
        assert_eq!(
            doc["draws"]["changed"][0]["b"]["pipelines"],
            json!(["unlit"])
        );
        let text = text("a", "b", &doc);
        assert!(
            text.contains("removed  crate in Scene: 1 draw: pbr"),
            "{text}"
        );
    }

    #[test]
    fn a_pixel_that_moves_past_the_tolerance_counts() {
        let t = |v: Vec<f32>| Texels {
            width: 3,
            height: 1,
            channels: 1,
            kind: read::Kind::Float,
            values: v,
        };
        let (n, max, _) = pixel_delta(&t(vec![0.0, 0.5, 1.0]), &t(vec![0.0, 0.5, 1.25]), 0.0);
        assert_eq!((n, max), (1, 0.25));
        let (n, ..) = pixel_delta(&t(vec![0.0, 0.5, 1.0]), &t(vec![0.0, 0.5, 1.25]), 0.3);
        assert_eq!(n, 0);
        let (n, ..) = pixel_delta(
            &t(vec![f32::NAN, 0.0, 0.0]),
            &t(vec![f32::NAN, 0.0, 0.0]),
            0.0,
        );
        assert_eq!(n, 0);
    }
}
