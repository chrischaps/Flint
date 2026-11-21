//! `flint frame replay`: draw a captured frame again (#1089, ADR 0126
//! decision 6), whole or with entities hidden, the scene pass stopped after
//! N draws, or passes switched off, and say how close the replay came.
//!
//! The host is `render::replay_host` (the authored scene loaded as every
//! `flint render` stage loads it, the captured world laid over it by name,
//! the renderer's state written back). The replay is written as a capture
//! of its own, so `flint frame show`, `diff` and `open` read it as they read
//! the original; this compares its final image and its scene pass's image
//! with the captured ones.

use super::read::{texels, Capture};
use crate::commands::render::replay_host::{Controls, ReplayHost};
use crate::output;
use anyhow::Result;
use clap::Args;
use flint_render::frame_capture::{FrameDescription, ImageDescription};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct ReplayArgs {
    /// The capture folder (one with `replay/`: `flint render
    /// --frame-capture`, or the player's)
    pub capture: PathBuf,
    /// Where to write the replayed frame, as a capture (default: beside
    /// the capture, `<name>.replay.flintframe`)
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// Draw the frame without this entity, by name (repeatable)
    #[arg(long, value_name = "NAME")]
    pub hide: Vec<String>,
    /// Stop a pass (the scene's, unless `--pass` names another) after its
    /// first N draws, counted as `flint frame show --draws --pass <name>`
    /// lists them
    #[arg(long, value_name = "N")]
    pub draws: Option<usize>,
    /// The pass `--draws` steps: `Scene` (the default), `Shadow cascade 0`
    /// to `2`, `Spot shadow`, `Style mask` or `Style albedo`
    #[arg(long, value_name = "PASS", requires = "draws")]
    pub pass: Option<String>,
    /// Draw the frame without this pass (`Bloom`, `SSAO`, `Shadows`, a
    /// style's; repeatable)
    #[arg(long, value_name = "PASS")]
    pub skip: Vec<String>,
    /// Mark this entity's pixels with the selection outline
    #[arg(long, value_name = "NAME")]
    pub outline: Option<String>,
    /// The scene, when it isn't where the capture says
    #[arg(long, value_name = "FILE")]
    pub scene: Option<PathBuf>,
    /// Schema directories, as `flint render` takes them
    #[arg(long, value_name = "DIR")]
    pub schemas: Vec<String>,
}

/// How a replayed image compares with the captured one.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Match {
    /// What was compared: `final` (the frame as shown) or `scene` (the
    /// scene pass's HDR image).
    pub image: &'static str,
    pub pixels: usize,
    /// Pixels that differ, 0 when byte-identical; `None` when it couldn't
    /// be compared.
    pub differing: Option<usize>,
    /// Why it couldn't, when it couldn't.
    pub why: Option<String>,
}

impl Match {
    pub fn identical(&self) -> bool {
        self.differing == Some(0)
    }

    /// `byte-identical`, `differs in 1,128 of 921,600 pixels`, or why not.
    pub fn words(&self) -> String {
        match (self.differing, &self.why) {
            (Some(0), _) => "byte-identical".into(),
            (Some(n), _) => format!(
                "differs in {} of {} pixels",
                thousands(n),
                thousands(self.pixels)
            ),
            (None, Some(why)) => format!("not compared: {why}"),
            (None, None) => "not compared".into(),
        }
    }
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The frame's finished image: the last pass image of the `surface`, from
/// the pass named `pass` when given (a replay ends at the renderer's last
/// pass; the player's capture has its HUD's `UI` after it, which a replay
/// doesn't draw).
fn final_image<'a>(
    d: &'a FrameDescription,
    pass: Option<&str>,
) -> Option<(&'a str, &'a ImageDescription)> {
    d.all_passes()
        .into_iter()
        .filter(|p| pass.is_none_or(|name| p.name == name))
        .flat_map(|p| p.images.iter().map(move |i| (p.name.as_str(), i)))
        .rfind(|(_, i)| i.target == "surface")
}

/// The scene pass's HDR image.
fn scene_image(d: &FrameDescription) -> Option<&ImageDescription> {
    d.pass("Scene")?
        .images
        .iter()
        .find(|i| i.target == "hdr_scene")
        .or_else(|| d.pass("Scene")?.images.first())
}

/// Compare an image of the two captures, texel by texel and exactly.
fn compare(
    what: &'static str,
    captured: &Capture,
    replayed: &Capture,
    images: (Option<&ImageDescription>, Option<&ImageDescription>),
) -> Match {
    let fail = |why: String| Match {
        image: what,
        pixels: 0,
        differing: None,
        why: Some(why),
    };
    let (Some(a), Some(b)) = images else {
        return fail("one of the two has no such image".into());
    };
    let read = |c: &Capture, i: &ImageDescription| texels(&c.file(i), i);
    match (read(captured, a), read(replayed, b)) {
        (Ok(x), Ok(y))
            if x.width == y.width && x.height == y.height && x.channels == y.channels =>
        {
            let (differing, _, _) = super::diff::pixel_delta(&x, &y, 0.0);
            Match {
                image: what,
                pixels: x.pixels(),
                differing: Some(differing),
                why: None,
            }
        }
        (Ok(x), Ok(y)) => fail(format!(
            "{}x{} against {}x{}",
            x.width, x.height, y.width, y.height
        )),
        (Err(e), _) | (_, Err(e)) => fail(format!("{e:#}")),
    }
}

/// Compare every image `pass` has in the capture with the replay's image of
/// the same target, step and layer (#1204): a shadow layer's depth, the
/// style mask's ids and veil, the albedo. One `Match`, summed over them.
fn compare_pass(pass: &str, captured: &Capture, replayed: &Capture) -> Match {
    let fail = |why: String| Match {
        image: "pass",
        pixels: 0,
        differing: None,
        why: Some(why),
    };
    let (Some(a), Some(b)) = (
        captured.description.pass(pass),
        replayed.description.pass(pass),
    ) else {
        return fail(format!("one of the two has no {pass} pass"));
    };
    if a.images.is_empty() {
        return fail(format!("the capture kept no image of {pass}"));
    }
    let (mut pixels, mut differing) = (0, 0);
    for image in &a.images {
        let same = |i: &&ImageDescription| {
            i.target == image.target && i.step == image.step && i.layer == image.layer
        };
        let m = compare(
            "pass",
            captured,
            replayed,
            (Some(image), b.images.iter().find(same)),
        );
        match m.differing {
            Some(n) => {
                pixels += m.pixels;
                differing += n;
            }
            None => return m,
        }
    }
    Match {
        image: "pass",
        pixels,
        differing: Some(differing),
        why: None,
    }
}

/// The comparison of the pass a replay steps (#1204), when it isn't the
/// scene's (which [`matches`] compares anyway).
pub fn stepped_match(captured: &Capture, replayed: &Capture, pass: &str) -> Option<Match> {
    (pass != "Scene").then(|| compare_pass(pass, captured, replayed))
}

/// The final image's and the scene pass's comparisons.
pub fn matches(captured: &Capture, replayed: &Capture) -> [Match; 2] {
    let replay_final = final_image(&replayed.description, None);
    // The captured image of the pass the replay ends on; else (an outline
    // the capture didn't draw) its last before the host's own `UI`
    let captured_final =
        final_image(&captured.description, replay_final.map(|(p, _)| p)).or_else(|| {
            captured
                .description
                .all_passes()
                .into_iter()
                .filter(|p| p.name != "UI")
                .flat_map(|p| p.images.iter().map(move |i| (p.name.as_str(), i)))
                .rfind(|(_, i)| i.target == "surface")
        });
    [
        compare(
            "final",
            captured,
            replayed,
            (captured_final.map(|(_, i)| i), replay_final.map(|(_, i)| i)),
        ),
        compare(
            "scene",
            captured,
            replayed,
            (
                scene_image(&captured.description),
                scene_image(&replayed.description),
            ),
        ),
    ]
}

/// Where a replay of `capture` goes by default: beside it.
pub fn default_out(capture: &Path) -> PathBuf {
    let dir = if capture.is_dir() {
        capture.to_path_buf()
    } else {
        capture.parent().unwrap_or(Path::new(".")).to_path_buf()
    };
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.trim_end_matches(".flintframe").to_string())
        .unwrap_or_else(|| "frame".into());
    dir.with_file_name(format!("{name}.replay.flintframe"))
}

pub fn run(args: ReplayArgs) -> Result<()> {
    let captured = Capture::open(&args.capture)?;
    let shown = Capture::shown(&args.capture);
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| default_out(&args.capture));
    let controls = Controls {
        hidden: args.hide.clone(),
        pass: args.pass.clone(),
        draws: args.draws,
        skip: args.skip.clone(),
        outline: args.outline.clone(),
    };
    // A pass that can't be stepped is refused before the scene loads
    let stepped = controls.stepped().map_err(anyhow::Error::msg)?.name();
    let mut host = ReplayHost::load(
        &captured.dir,
        &captured.description,
        args.scene.as_deref(),
        &args.schemas,
    )?;
    let drawn = host.draw(&controls, &out)?;
    let replayed = Capture::open(&out)?;
    let [final_match, scene_match] = matches(&captured, &replayed);
    let pass_match = controls
        .draws
        .and_then(|_| stepped_match(&captured, &replayed, stepped));
    let asked = controls != Controls::default();
    let out_shown = Capture::shown(&out);

    let draws_of = |name: &str| captured.description.pass(name).map_or(0, |p| p.draws.len());
    let scene_draws = draws_of("Scene");
    let pass_draws = draws_of(stepped);
    let mut text = format!("Replayed {shown} into {out_shown}\n");
    let gpu: f32 = replayed
        .description
        .all_passes()
        .iter()
        .filter_map(|p| p.gpu_ms)
        .sum();
    if asked {
        let mut what = Vec::new();
        if !controls.hidden.is_empty() {
            what.push(format!("without {}", controls.hidden.join(", ")));
        }
        if let Some(n) = controls.draws {
            what.push(format!("the {stepped} pass to draw {n} of {pass_draws}"));
        }
        if !controls.skip.is_empty() {
            what.push(format!("without the {} pass", controls.skip.join(", ")));
        }
        if let Some(name) = &controls.outline {
            what.push(format!("{name} outlined"));
        }
        text.push_str(&format!("  as asked: {}\n", what.join("; ")));
    }
    text.push_str(&format!("  final image: {}\n", final_match.words()));
    text.push_str(&format!("  scene pass:  {}\n", scene_match.words()));
    if let Some(m) = &pass_match {
        text.push_str(&format!("  {stepped}: {}\n", m.words()));
    }
    if gpu > 0.0 {
        text.push_str(&format!("  GPU: {gpu:.2} ms across its passes\n"));
    }
    let differs = !final_match.identical() || !scene_match.identical();
    if differs && !host.fresh_only.is_empty() {
        text.push_str("  drawn fresh, not as captured:\n");
        for line in &host.fresh_only {
            text.push_str(&format!("    {line}\n"));
        }
    }
    // What each style did with the captured history (#1136)
    for line in &host.history_notes {
        text.push_str(&format!("  history: {line}\n"));
    }
    let assets_ok = host.assets.iter().filter(|a| a.same_bytes()).count();
    if !host.assets.is_empty() {
        text.push_str(&format!(
            "  assets: {assets_ok} of {} as captured\n",
            host.assets.len()
        ));
    }
    for note in host.notes.iter().chain(&drawn.refused) {
        text.push_str(&format!("  note: {note}\n"));
    }
    if !host.skippable.is_empty() && !asked {
        let names: Vec<&str> = host.skippable.iter().map(|s| s.pass.as_str()).collect();
        text.push_str(&format!("  --skip takes: {}\n", names.join(", ")));
    }

    let json = serde_json::json!({
        "capture": shown,
        "replay": out_shown,
        "controls": {
            "hidden": controls.hidden,
            "pass": controls.draws.map(|_| stepped),
            "draws": controls.draws,
            "skip": controls.skip,
            "outline": controls.outline,
        },
        "scene_draws": scene_draws,
        "pass_draws": pass_draws,
        "final": final_match,
        "scene": scene_match,
        "pass": pass_match,
        "identical": final_match.identical()
            && scene_match.identical()
            && pass_match.as_ref().is_none_or(Match::identical),
        "gpu_ms": gpu,
        "fresh_only": host.fresh_only,
        "history": host.history_notes,
        "assets": host.assets,
        "notes": host.notes,
        "refused": drawn.refused,
        "skippable": host.skippable,
    });
    output::emit(&output::Answer::new(text.trim_end().to_string(), json))
}
