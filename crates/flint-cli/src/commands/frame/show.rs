//! `flint frame show`: a capture's summary, and its passes, draws, targets
//! and hitch window. Each view is built once as JSON and its text is
//! written from that document, so the two forms carry the same numbers.

use super::read::Capture;
use super::table::Table;
use super::{bytes, name_matches, passes, DrawSort, PassAt, ShowArgs};
use crate::output::{self, Answer};
use anyhow::{bail, Result};
use flint_render::frame_capture::{DrawDescription, FrameDescription};
use serde_json::{json, Value};
use std::fmt::Write as _;

pub fn run(args: ShowArgs) -> Result<()> {
    let capture = Capture::open(&args.capture)?;
    let shown = Capture::shown(&args.capture);
    let d = &capture.description;
    let draws = args.draws
        || args.pass.is_some()
        || args.entity.is_some()
        || args.sort.is_some()
        || args.culled;
    let any_view = args.passes || draws || args.targets || args.hitch;

    let mut text = String::new();
    let mut doc = serde_json::Map::new();
    doc.insert("path".into(), json!(shown));
    if !any_view {
        let summary = summary(d, args.top);
        text.push_str(&summary_text(&shown, &summary));
        doc.insert("summary".into(), summary);
    }
    if args.passes {
        let rows = pass_rows(d);
        text.push_str(&passes_text(&rows));
        doc.insert("passes".into(), Value::Array(rows));
    }
    if draws {
        let rows = draw_rows(d, &args)?;
        text.push_str(&draws_text(&rows, args.culled));
        doc.insert("draws".into(), Value::Array(rows));
    }
    if args.targets {
        let rows = target_rows(d);
        text.push_str(&targets_text(&rows));
        doc.insert("targets".into(), Value::Array(rows));
    }
    if args.hitch {
        let hitch = hitch(d);
        text.push_str(&hitch_text(&hitch));
        doc.insert("hitch".into(), hitch);
    }
    output::emit(&Answer::new(text, Value::Object(doc)))
}

// ── The summary ──

/// The text `flint frame show <capture>` prints: what `flint frame open`'s
/// Copy summary puts on the clipboard.
pub(crate) fn summary_of(d: &FrameDescription, shown: &str) -> String {
    summary_text(shown, &summary(d, 10))
}

/// A pass's own draws and their triangles.
fn pass_totals(p: &PassAt) -> (usize, u64) {
    let draws = p.pass.draws.len();
    let tris = p.pass.draws.iter().map(|d| d.triangles as u64).sum();
    (draws, tris)
}

/// A pass's draws with those of the passes inside it (`Shadows` and its
/// layers), as its GPU time counts them.
fn inclusive_totals(pass: &flint_render::frame_capture::PassDescription) -> (usize, u64) {
    let mut draws = pass.draws.len();
    let mut tris: u64 = pass.draws.iter().map(|d| d.triangles as u64).sum();
    for inner in &pass.passes {
        let (d, t) = inclusive_totals(inner);
        draws += d;
        tris += t;
    }
    (draws, tris)
}

fn summary(d: &FrameDescription, top: usize) -> Value {
    let all = passes(d);
    let mut timed: Vec<&PassAt> = all.iter().filter(|p| p.pass.gpu_ms.is_some()).collect();
    timed.sort_by(|a, b| {
        b.pass
            .gpu_ms
            .partial_cmp(&a.pass.gpu_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top_passes: Vec<Value> = timed
        .iter()
        .take(top)
        .map(|p| {
            let (draws, tris) = inclusive_totals(p.pass);
            json!({
                "index": p.index,
                "pass": p.key(),
                "gpu_ms": p.pass.gpu_ms,
                "draws": draws,
                "triangles": tris,
            })
        })
        .collect();
    let s = &d.stats;
    let f = &d.frame;
    let cpu_frame_ms = (s.frame_time_ms > 0.0).then_some(s.frame_time_ms);
    json!({
        "format": d.format,
        "version": d.version,
        "written_by": d.written_by,
        "captured_at": d.captured_at,
        "engine": d.engine,
        "project": d.project,
        "scene": d.scene,
        "trigger": d.trigger,
        "notes": d.notes,
        "camera": d.camera,
        "frame": {
            "index": f.index,
            "game_time": f.game_time,
            "dt": f.dt,
            "size": f.size,
            "scene_size": f.scene_size,
            "render_scale": f.render_scale,
            "msaa": f.msaa,
            "surface_format": f.surface_format,
            "adapter": f.adapter,
            "backend": f.backend,
            "present_mode": f.present_mode,
            "debug_mode": f.debug_mode,
            "capturing": f.capturing,
        },
        "time": {
            "gpu_frame_ms": f.gpu_frame_ms,
            "cpu_frame_ms": d.cpu.as_ref().and_then(|c| c.frame.as_ref()).map(|f| f.ms).or(cpu_frame_ms),
            "cpu_phases_ms": d.cpu.as_ref().and_then(|c| c.frame.as_ref()).map(|f| &f.phases_ms),
        },
        "draws": {
            "draw_calls": s.draw_calls,
            "triangles": s.triangles,
            "shadow_draw_calls": s.shadow_draw_calls,
            "shadow_triangles": s.shadow_triangles,
            "culled_draws": s.culled_draws,
            "lod_culled_draws": s.lod_culled_draws,
            "skinned_draws": s.skinned_draws,
            "transparent_draws": s.transparent_draws,
            "particle_draws": s.particle_draws,
            "particle_instances": s.particle_instances,
            "grass_instances": s.grass_instances,
        },
        "passes": {
            "total": all.len(),
            "ran": all.iter().filter(|p| p.pass.ran).count(),
            "timed": timed.len(),
        },
        "top_passes": top_passes,
        "memory": {
            "target_bytes": d.memory.target_bytes,
            "style_target_bytes": d.memory.style_target_bytes,
            "targets": d.targets.len(),
        },
        "images": d.capture.as_ref().map(|c| json!({
            "images": c.images,
            "file_bytes": c.file_bytes,
            "raw_bytes": c.raw_bytes,
            "untapped": c.untapped,
        })),
        "styles": d.frame.styles,
        "lights": {
            "kept": d.lights.kept.len(),
            "left_out": d.lights.left_out.len(),
            "default_lights": d.lights.default_lights,
        },
        "panels": d.panels.iter().map(|p| &p.name).collect::<Vec<_>>(),
    })
}

fn ms(v: &Value) -> String {
    v.as_f64().map_or("-".into(), |m| format!("{m:.3}"))
}

fn count(n: &Value) -> String {
    flint_render::format_count(n.as_u64().unwrap_or(0).min(u32::MAX as u64) as u32)
}

/// The frame's camera in a line (#1176): `perspective, fov 60°, near 0.1 m,
/// far 1000 m, at (0, 2, 8) looking at (0, 1, 0)`. `None` for a capture
/// that names none (written before it).
fn camera_words(c: &Value) -> Option<String> {
    let c = c.as_object()?;
    let num = |k: &str| c.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let point = |k: &str| {
        let v: Vec<String> = c
            .get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|x| metres(x.as_f64().unwrap_or(0.0)))
                    .collect()
            })
            .unwrap_or_default();
        format!("({})", v.join(", "))
    };
    let lens = if c.get("orthographic").and_then(Value::as_bool) == Some(true) {
        // ortho_height is the half-height; 0 sizes it from the distance
        let half = match num("ortho_height") {
            h if h > 0.0 => h,
            _ => num("distance") * (num("fov").to_radians() / 2.0).tan(),
        };
        format!("orthographic, {} m tall", metres(half * 2.0))
    } else {
        format!("perspective, fov {}°", metres(num("fov")))
    };
    Some(format!(
        "{lens}, near {} m, far {} m, at {} looking at {}",
        metres(num("near")),
        metres(num("far")),
        point("position"),
        point("target")
    ))
}

/// A length as few digits as say it: `0.1`, `12.5`, `1000`.
fn metres(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

fn summary_text(shown: &str, s: &Value) -> String {
    let mut out = String::new();
    let line = |out: &mut String, label: &str, value: String| {
        let _ = writeln!(out, "{label:<9}{value}");
    };
    let mut written = format!("{} v{}", s["format"].as_str().unwrap_or(""), s["version"]);
    if let Some(by) = s["written_by"].as_str() {
        let _ = write!(written, ", written by {by}");
    }
    if let Some(at) = s["captured_at"].as_str() {
        let _ = write!(written, " at {at}");
    }
    if let Some(v) = s["engine"]["version"].as_str() {
        let e = &s["engine"];
        let _ = write!(
            written,
            ", engine {v}{}",
            super::commit_words(
                e["commit"].as_str(),
                e["subdir"].as_str(),
                e["dirty"].as_bool()
            )
        );
        // A game's own checkout, when it isn't the engine's (#1290)
        if let Some(c) = s["project"]["commit"]
            .as_str()
            .filter(|c| Some(*c) != e["commit"].as_str())
        {
            let short: String = c.chars().take(10).collect();
            let _ = write!(written, ", project at {short}");
        }
    }
    line(&mut out, "Capture", format!("{shown}  ({written})"));
    let scene = &s["scene"];
    if scene.is_object() {
        let path = scene["path"].as_str().unwrap_or("");
        let named = scene["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .map_or(path.to_string(), |n| format!("{n}  ({path})"));
        line(&mut out, "Scene", named);
    }
    if let Some(why) = s["trigger"]["detail"].as_str().filter(|d| !d.is_empty()) {
        line(&mut out, "Trigger", why.to_string());
    }
    let f = &s["frame"];
    let mut at = Vec::new();
    if let Some(t) = f["game_time"].as_f64() {
        at.push(format!("t = {t:.3} s"));
    }
    if let Some(i) = f["index"].as_u64() {
        at.push(format!("frame {i}"));
    }
    if let Some(dt) = f["dt"].as_f64() {
        at.push(format!("dt {:.2} ms", dt * 1000.0));
    }
    if !at.is_empty() {
        line(&mut out, "At", at.join(", "));
    }
    let adapter = f["adapter"].as_str().unwrap_or("(not said)");
    // The adapter line names its backend already, as a rule
    let backend = f["backend"]
        .as_str()
        .filter(|b| !adapter.contains(b))
        .map_or(String::new(), |b| format!(" on {b}"));
    let present = f["present_mode"]
        .as_str()
        .map_or(" (headless)".to_string(), |p| format!(", present {p}"));
    line(&mut out, "Adapter", format!("{adapter}{backend}{present}"));
    let size = |v: &Value| format!("{}x{}", v[0], v[1]);
    let mut frame = format!(
        "{}, scene {} (scale {}), msaa {}, {}",
        size(&f["size"]),
        size(&f["scene_size"]),
        f["render_scale"],
        f["msaa"],
        f["surface_format"].as_str().unwrap_or("")
    );
    let shading = f["debug_mode"].as_str().unwrap_or("");
    if !shading.is_empty() && !["pbr", "normal"].contains(&shading.to_ascii_lowercase().as_str()) {
        let _ = write!(frame, ", debug {}", f["debug_mode"].as_str().unwrap_or(""));
    }
    line(&mut out, "Frame", frame);
    if let Some(c) = camera_words(&s["camera"]) {
        line(&mut out, "Camera", c);
    }
    let t = &s["time"];
    let mut time = match t["gpu_frame_ms"].as_f64() {
        Some(g) => format!("GPU {g:.3} ms"),
        None => "GPU untimed".to_string(),
    };
    if let Some(c) = t["cpu_frame_ms"].as_f64() {
        let _ = write!(time, ", CPU frame {c:.3} ms");
    }
    if let Some(phases) = t["cpu_phases_ms"].as_object() {
        let list: Vec<String> = phases
            .iter()
            .map(|(k, v)| format!("{k} {}", ms(v)))
            .collect();
        if !list.is_empty() {
            let _ = write!(time, "; phases: {}", list.join(", "));
        }
    }
    if f["capturing"].as_bool() == Some(true) && t["gpu_frame_ms"].is_number() {
        time.push_str(" (holds the capture's copies)");
    }
    line(&mut out, "Time", time);
    let dr = &s["draws"];
    let mut draws = format!(
        "{} draws, {} triangles; shadows {} draws, {} triangles",
        dr["draw_calls"],
        count(&dr["triangles"]),
        dr["shadow_draw_calls"],
        count(&dr["shadow_triangles"])
    );
    if dr["culled_draws"].as_u64().unwrap_or(0) > 0 {
        let _ = write!(draws, "; {} culled", dr["culled_draws"]);
    }
    line(&mut out, "Draws", draws);
    let p = &s["passes"];
    line(
        &mut out,
        "Passes",
        format!("{} of {} ran, {} timed", p["ran"], p["total"], p["timed"]),
    );
    let m = &s["memory"];
    let mut memory = format!(
        "{} in {} targets",
        bytes(m["target_bytes"].as_u64().unwrap_or(0)),
        m["targets"]
    );
    if m["style_target_bytes"].as_u64().unwrap_or(0) > 0 {
        let _ = write!(
            memory,
            " ({} the style chain's)",
            bytes(m["style_target_bytes"].as_u64().unwrap_or(0))
        );
    }
    line(&mut out, "Memory", memory);
    let i = &s["images"];
    if i.is_object() {
        let mut images = format!(
            "{} images, {} on disk",
            i["images"],
            bytes(i["file_bytes"].as_u64().unwrap_or(0))
        );
        let untapped = i["untapped"].as_array().map_or(0, Vec::len);
        if untapped > 0 {
            let _ = write!(images, "; {untapped} written targets not read");
        }
        line(&mut out, "Images", images);
    } else {
        line(
            &mut out,
            "Images",
            "none (a description without images)".into(),
        );
    }
    if let Some(styles) = s["styles"].as_array().filter(|s| !s.is_empty()) {
        let list: Vec<String> = styles
            .iter()
            .map(|st| {
                format!(
                    "{} ({})",
                    st["name"].as_str().unwrap_or(""),
                    st["stage"].as_str().unwrap_or("")
                )
            })
            .collect();
        line(&mut out, "Styles", list.join(" > "));
    }
    let l = &s["lights"];
    let mut lights = format!("{} drawn", l["kept"]);
    if l["left_out"].as_u64().unwrap_or(0) > 0 {
        let _ = write!(lights, ", {} past the caps", l["left_out"]);
    }
    if l["default_lights"].as_bool() == Some(true) {
        lights.push_str(" (the built-in pair: the scene has no usable light)");
    }
    line(&mut out, "Lights", lights);
    for note in s["notes"].as_array().into_iter().flatten() {
        line(&mut out, "Note", note.as_str().unwrap_or("").to_string());
    }
    if let Some(top) = s["top_passes"].as_array().filter(|t| !t.is_empty()) {
        let _ = writeln!(out, "\nMost expensive passes (GPU ms):");
        let mut t = Table::new(&[">#", "pass", ">gpu ms", ">draws", ">triangles"]);
        for p in top {
            t.row(vec![
                format!("{:02}", p["index"].as_u64().unwrap_or(0)),
                p["pass"].as_str().unwrap_or("").to_string(),
                ms(&p["gpu_ms"]),
                p["draws"].to_string(),
                count(&p["triangles"]),
            ]);
        }
        out.push_str(&t.render("  "));
    }
    out.push_str(
        "\nViews: --passes, --draws [--pass P] [--entity E] [--sort tris|gpu] [--culled], --targets, --hitch\n",
    );
    out
}

// ── Passes ──

fn pass_rows(d: &FrameDescription) -> Vec<Value> {
    passes(d)
        .iter()
        .map(|p| {
            let (draws, tris) = pass_totals(p);
            json!({
                "index": p.index,
                "pass": p.pass.name,
                "key": p.key(),
                "depth": p.depth,
                "ran": p.pass.ran,
                "skipped": p.pass.skipped,
                "timed": p.pass.timed,
                "gpu_ms": p.pass.gpu_ms,
                "draws": draws,
                "culled": p.pass.culled.len(),
                "triangles": tris,
                "reads": p.pass.reads,
                "writes": p.pass.writes,
                "also": p.pass.also,
                "images": p.pass.images.iter().map(|i| &i.file).collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn passes_text(rows: &[Value]) -> String {
    let mut t = Table::new(&[
        ">#",
        "pass",
        ">gpu ms",
        ">draws",
        ">triangles",
        "writes / why not",
    ]);
    for r in rows {
        let depth = r["depth"].as_u64().unwrap_or(0) as usize;
        let name = format!("{}{}", "  ".repeat(depth), r["key"].as_str().unwrap_or(""));
        let ran = r["ran"].as_bool() == Some(true);
        let last = if ran {
            let writes: Vec<&str> = r["writes"]
                .as_array()
                .map(|w| w.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            writes.join(", ")
        } else {
            format!("skipped: {}", r["skipped"].as_str().unwrap_or("not drawn"))
        };
        let gpu = if !ran {
            String::new()
        } else if r["timed"].as_bool() == Some(true) {
            ms(&r["gpu_ms"])
        } else {
            "untimed".into()
        };
        let draws = r["draws"].as_u64().unwrap_or(0);
        t.row(vec![
            format!("{:02}", r["index"].as_u64().unwrap_or(0)),
            name,
            gpu,
            if draws > 0 {
                draws.to_string()
            } else {
                String::new()
            },
            if draws > 0 {
                count(&r["triangles"])
            } else {
                String::new()
            },
            last,
        ]);
    }
    let ran = rows
        .iter()
        .filter(|r| r["ran"].as_bool() == Some(true))
        .count();
    format!("Passes: {ran} of {} ran\n{}", rows.len(), t.render("  "))
}

// ── Draws ──

fn draw_row(p: &PassAt, d: &DrawDescription) -> Value {
    json!({
        "pass_index": p.index,
        "pass": p.key(),
        "entity": d.entity,
        "entity_id": d.entity_id,
        "kind": d.kind,
        "layer": d.layer,
        "blend": d.blend,
        "vertex": d.vertex,
        "pipeline": d.pipeline,
        "mesh": d.mesh,
        "material": d.material,
        "triangles": d.triangles,
        "instances": d.instances,
        "casts_shadow": d.casts_shadow,
        "double_sided": d.double_sided,
        "culled": d.culled,
        "gpu_ms": d.gpu_ms,
    })
}

fn draw_rows(d: &FrameDescription, args: &ShowArgs) -> Result<Vec<Value>> {
    let all = passes(d);
    let chosen: Vec<&PassAt> = match &args.pass {
        Some(name) => {
            let found: Vec<&PassAt> = all
                .iter()
                .filter(|p| {
                    p.pass.name.eq_ignore_ascii_case(name) || p.key().eq_ignore_ascii_case(name)
                })
                .collect();
            if found.is_empty() {
                let names: Vec<&str> = all.iter().map(|p| p.pass.name.as_str()).collect();
                let hint = flint_core::diagnostic::did_you_mean(name, names.iter().copied())
                    .map_or(String::new(), |h| format!(": {h}"));
                bail!("no pass is named {name:?}{hint} (`flint frame show <capture> --passes` lists them)");
            }
            found
        }
        None => all.iter().collect(),
    };
    let mut rows = Vec::new();
    for p in &chosen {
        let culled = args
            .culled
            .then_some(p.pass.culled.iter())
            .into_iter()
            .flatten();
        for draw in p.pass.draws.iter().chain(culled) {
            if let Some(pattern) = &args.entity {
                if !draw
                    .entity
                    .as_deref()
                    .is_some_and(|e| name_matches(pattern, e))
                {
                    continue;
                }
            }
            rows.push(draw_row(p, draw));
        }
    }
    if rows.is_empty() {
        if let Some(pattern) = &args.entity {
            let mut names: Vec<&str> = all
                .iter()
                .flat_map(|p| p.pass.draws.iter().chain(&p.pass.culled))
                .filter_map(|d| d.entity.as_deref())
                .collect();
            names.sort_unstable();
            names.dedup();
            let culled_too = all.iter().any(|p| {
                p.pass.culled.iter().any(|d| {
                    d.entity
                        .as_deref()
                        .is_some_and(|e| name_matches(pattern, e))
                })
            });
            if culled_too && !args.culled {
                bail!("{pattern:?} draws nothing in this frame, but a cull set it aside: add --culled to see why");
            }
            let hint = flint_core::diagnostic::did_you_mean(pattern, names.iter().copied())
                .map_or(String::new(), |h| format!(": {h}"));
            bail!("no draw is of an entity named {pattern:?}{hint}");
        }
    }
    match args.sort {
        Some(DrawSort::Tris) => {
            rows.sort_by_key(|r| std::cmp::Reverse(r["triangles"].as_u64().unwrap_or(0)));
        }
        Some(DrawSort::Gpu) => {
            let ms = |r: &Value| r["gpu_ms"].as_f64().unwrap_or(-1.0);
            rows.sort_by(|a, b| ms(b).total_cmp(&ms(a)));
        }
        Some(DrawSort::Pass) | None => {}
    }
    Ok(rows)
}

fn draws_text(rows: &[Value], culled: bool) -> String {
    // A replay's draw times (#1203), when any draw has one
    let timed = rows.iter().any(|r| r["gpu_ms"].is_number());
    let mut header = vec![
        "pass",
        "entity",
        "kind",
        "layer",
        "pipeline",
        "mesh",
        ">triangles",
        ">inst",
    ];
    if timed {
        header.push(">GPU ms");
    }
    if culled {
        header.push("culled");
    }
    let mut t = Table::new(&header);
    let s = |v: &Value| v.as_str().unwrap_or("-").to_string();
    for r in rows {
        let mut row = vec![
            s(&r["pass"]),
            s(&r["entity"]),
            s(&r["kind"]),
            s(&r["layer"]),
            s(&r["pipeline"]),
            s(&r["mesh"]),
            count(&r["triangles"]),
            r["instances"].to_string(),
        ];
        if timed {
            row.push(
                r["gpu_ms"]
                    .as_f64()
                    .map_or("-".into(), |ms| format!("{ms:.3}")),
            );
        }
        if culled {
            row.push(r["culled"].as_str().unwrap_or("").to_string());
        }
        t.row(row);
    }
    let tris: u64 = rows
        .iter()
        .filter(|r| r["culled"].is_null())
        .map(|r| r["triangles"].as_u64().unwrap_or(0))
        .sum();
    let drawn = rows.iter().filter(|r| r["culled"].is_null()).count();
    let mut head = format!(
        "Draws: {drawn}, {} triangles",
        flint_render::format_count(tris.min(u32::MAX as u64) as u32)
    );
    if timed {
        let ms: f64 = rows.iter().filter_map(|r| r["gpu_ms"].as_f64()).sum();
        let _ = write!(head, ", {ms:.3} GPU ms inside them");
    }
    if culled {
        let _ = write!(head, "; {} culled", rows.len() - drawn);
    }
    format!("{head}\n{}", t.render("  "))
}

// ── Targets ──

fn target_rows(d: &FrameDescription) -> Vec<Value> {
    let all = passes(d);
    d.targets
        .iter()
        .map(|t| {
            let by = |f: &dyn Fn(&PassAt) -> bool| -> Vec<String> {
                all.iter().filter(|p| f(p)).map(PassAt::key).collect()
            };
            let images: Vec<&str> = all
                .iter()
                .flat_map(|p| p.pass.images.iter())
                .filter(|i| i.target == t.id)
                .map(|i| i.file.as_str())
                .collect();
            json!({
                "id": t.id,
                "format": t.format,
                "size": t.size,
                "layers": t.layers,
                "samples": t.samples,
                "bytes": t.bytes,
                "written_by": by(&|p| p.pass.writes.contains(&t.id)),
                "read_by": by(&|p| p.pass.reads.contains(&t.id)),
                "images": images,
            })
        })
        .collect()
}

fn targets_text(rows: &[Value]) -> String {
    let mut t = Table::new(&[
        "target",
        "format",
        "size",
        ">memory",
        ">images",
        "written by",
    ]);
    for r in rows {
        let mut size = format!("{}x{}", r["size"][0], r["size"][1]);
        if r["layers"].as_u64().unwrap_or(1) > 1 {
            let _ = write!(size, "x{}", r["layers"]);
        }
        if r["samples"].as_u64().unwrap_or(1) > 1 {
            let _ = write!(size, " {}x", r["samples"]);
        }
        let writers: Vec<&str> = r["written_by"]
            .as_array()
            .map(|w| w.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        t.row(vec![
            r["id"].as_str().unwrap_or("").to_string(),
            r["format"].as_str().unwrap_or("").to_string(),
            size,
            bytes(r["bytes"].as_u64().unwrap_or(0)),
            r["images"].as_array().map_or(0, Vec::len).to_string(),
            writers.join(", "),
        ]);
    }
    let total: u64 = rows.iter().map(|r| r["bytes"].as_u64().unwrap_or(0)).sum();
    format!(
        "Targets: {}, {}\n{}",
        rows.len(),
        bytes(total),
        t.render("  ")
    )
}

// ── The hitch window ──

fn hitch(d: &FrameDescription) -> Value {
    match &d.cpu {
        Some(cpu) => json!({
            "hitch": cpu.hitch,
            "frame": cpu.frame,
            "window": cpu.window,
            "scripts_timed": cpu.scripts_timed,
            "missing": cpu.missing,
        }),
        None => json!({
            "hitch": null,
            "window": [],
            "why": format!(
                "no frame history: {} keeps none (the player's captures carry it)",
                d.written_by.as_deref().unwrap_or("its writer")
            ),
        }),
    }
}

/// A frame note as one bracketed word or two (`[model load 12.0 ms]`).
fn note_text(n: &Value) -> String {
    let mut text = n["kind"].as_str().unwrap_or("").to_string();
    if let Some(detail) = n["detail"].as_str().filter(|d| !d.is_empty()) {
        if !text.is_empty() {
            text.push_str(": ");
        }
        text.push_str(detail);
    }
    if let Some(ms) = n["ms"].as_f64() {
        let _ = write!(text, " {ms:.1} ms");
    }
    format!("[{text}]")
}

fn hitch_text(h: &Value) -> String {
    if let Some(why) = h["why"].as_str() {
        return format!("Hitch: {why}\n");
    }
    let mut out = String::new();
    match h["hitch"].as_object() {
        Some(hitch) => {
            let v = |k: &str| hitch.get(k).unwrap_or(&Value::Null);
            let _ = writeln!(
                out,
                "Hitch: frame {}, {} ms against a median of {} ms ({}): {}",
                v("frame"),
                ms(v("ms")),
                ms(v("median_ms")),
                v("class").as_str().unwrap_or("?"),
                v("cause").as_str().unwrap_or("?")
            );
            if let Some(phase) = v("phase").as_str() {
                let _ = writeln!(out, "  grew most: {phase} ({} ms)", ms(v("phase_ms")));
            }
            if let Some(script) = v("script").as_str() {
                let _ = writeln!(
                    out,
                    "  longest script: {script} ({} ms)",
                    ms(v("script_ms"))
                );
            }
            out.push_str(
                "  The capture is of the frame after it: a hitch is over by the time it is seen.\n",
            );
        }
        None => out.push_str("Hitch: none asked for this capture\n"),
    }
    if let Some(missing) = h["missing"].as_str() {
        let _ = writeln!(out, "  no window: {missing}");
        return out;
    }
    let window = h["window"].as_array().cloned().unwrap_or_default();
    if window.is_empty() {
        out.push_str("  (no window of frames)\n");
        return out;
    }
    let mut t = Table::new(&[
        ">frame",
        ">t s",
        ">ms",
        ">gpu ms",
        ">steps",
        "",
        "phases / notes",
    ]);
    for f in &window {
        let mut mark = String::new();
        if f["hitch"].as_bool() == Some(true) {
            mark.push_str("HITCH");
        }
        if f["capturing"].as_bool() == Some(true) {
            mark.push_str(if mark.is_empty() {
                "capture"
            } else {
                " capture"
            });
        }
        let mut detail: Vec<String> = f["phases_ms"]
            .as_object()
            .map(|p| p.iter().map(|(k, v)| format!("{k} {}", ms(v))).collect())
            .unwrap_or_default();
        detail.extend(f["notes"].as_array().into_iter().flatten().map(note_text));
        t.row(vec![
            f["index"].to_string(),
            f["game_time"]
                .as_f64()
                .map_or("-".into(), |t| format!("{t:.3}")),
            ms(&f["ms"]),
            ms(&f["gpu_ms"]),
            f["fixed_steps"].to_string(),
            mark,
            detail.join(", "),
        ]);
    }
    out.push_str(&t.render("  "));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_render::frame_capture::{
        CpuDescription, CpuFrame, CpuNote, HitchDescription, PassDescription,
    };

    fn frame() -> FrameDescription {
        let mut d = FrameDescription::default();
        let draw = |entity: &str, tris: u32| DrawDescription {
            entity: Some(entity.into()),
            kind: "Static".into(),
            pipeline: "pbr".into(),
            triangles: tris,
            instances: 1,
            ..Default::default()
        };
        d.passes = vec![
            PassDescription {
                name: "Scene".into(),
                ran: true,
                timed: true,
                gpu_ms: Some(1.5),
                draws: vec![draw("crate", 12), draw("ground", 2), draw("tree_oak", 900)],
                culled: vec![DrawDescription {
                    culled: Some("frustum".into()),
                    ..draw("far_rock", 40)
                }],
                ..Default::default()
            },
            PassDescription {
                name: "Bloom".into(),
                skipped: Some("bloom_enabled is false".into()),
                ..Default::default()
            },
        ];
        d
    }

    fn args() -> ShowArgs {
        ShowArgs {
            capture: "x".into(),
            passes: false,
            draws: true,
            pass: None,
            entity: None,
            sort: None,
            culled: false,
            targets: false,
            hitch: false,
            top: 10,
        }
    }

    #[test]
    fn draws_filter_by_entity_and_sort_by_triangles() {
        let d = frame();
        let mut a = args();
        a.entity = Some("tree_*".into());
        let rows = draw_rows(&d, &a).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["entity"], "tree_oak");
        let mut a = args();
        a.sort = Some(DrawSort::Tris);
        let rows = draw_rows(&d, &a).unwrap();
        assert_eq!(rows[0]["entity"], "tree_oak");
        assert_eq!(rows[2]["entity"], "ground");
    }

    #[test]
    fn draws_sort_by_gpu_time_untimed_last_and_show_the_column() {
        let mut d = frame();
        let scene = &mut d.passes[0];
        scene.draws[0].gpu_ms = Some(0.02);
        scene.draws[2].gpu_ms = Some(0.31);
        let mut a = args();
        a.sort = Some(DrawSort::Gpu);
        let rows = draw_rows(&d, &a).unwrap();
        let order: Vec<&str> = rows.iter().map(|r| r["entity"].as_str().unwrap()).collect();
        assert_eq!(order, ["tree_oak", "crate", "ground"]);
        let text = draws_text(&rows, false);
        assert!(text.contains("GPU ms"), "{text}");
        assert!(text.contains("0.310"), "{text}");
        assert!(text.contains("0.330 GPU ms inside them"), "{text}");
        // An untimed frame has no column
        let rows = draw_rows(&frame(), &args()).unwrap();
        assert!(!draws_text(&rows, false).contains("GPU ms"));
    }

    #[test]
    fn a_culled_entity_says_to_ask_for_culls() {
        let d = frame();
        let mut a = args();
        a.entity = Some("far_rock".into());
        let e = draw_rows(&d, &a).unwrap_err().to_string();
        assert!(e.contains("--culled"), "{e}");
        a.culled = true;
        let rows = draw_rows(&d, &a).unwrap();
        assert_eq!(rows[0]["culled"], "frustum");
        let mut a = args();
        a.entity = Some("crat".into());
        let e = draw_rows(&d, &a).unwrap_err().to_string();
        assert!(e.contains("did you mean `crate`"), "{e}");
    }

    #[test]
    fn the_summary_ranks_passes_by_gpu_time_and_counts_skips() {
        let s = summary(&frame(), 10);
        assert_eq!(s["passes"]["total"], 2);
        assert_eq!(s["passes"]["ran"], 1);
        assert_eq!(s["top_passes"][0]["pass"], "Scene");
        assert_eq!(s["top_passes"][0]["triangles"], 914);
        let rows = pass_rows(&frame());
        assert!(passes_text(&rows).contains("skipped: bloom_enabled is false"));
    }

    #[test]
    fn a_hitch_window_shows_its_frames() {
        let mut d = frame();
        assert!(hitch_text(&hitch(&d)).contains("no frame history"));
        d.cpu = Some(CpuDescription {
            window: vec![
                CpuFrame {
                    index: 9,
                    ms: 48.0,
                    hitch: true,
                    notes: vec![CpuNote {
                        kind: "model load".into(),
                        detail: "barrel".into(),
                        ms: Some(31.0),
                        depth: 0,
                    }],
                    ..Default::default()
                },
                CpuFrame {
                    index: 10,
                    ms: 17.0,
                    capturing: true,
                    ..Default::default()
                },
            ],
            hitch: Some(HitchDescription {
                frame: 9,
                ms: 48.0,
                median_ms: 16.7,
                class: "player".into(),
                cause: "model load".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
        let text = hitch_text(&hitch(&d));
        assert!(text.contains("HITCH"), "{text}");
        assert!(text.contains("[model load: barrel 31.0 ms]"), "{text}");
        assert!(text.contains("Hitch: frame 9, 48.000 ms"), "{text}");
    }
}
