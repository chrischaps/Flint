//! `flint render-suite <file>.goldens.toml` (#271): render a list of
//! scenes with `flint render`, each with its own flags, and compare every
//! image with a checked-in golden PNG. The suite fails when any case moves
//! past its tolerance, so a change to the stylize framework (or anything
//! under it) can't silently shift a look.
//!
//! ```toml
//! [suite]
//! name = "stylize"
//! root = "../../../.."            # cases run from here (scene paths are relative to it)
//! width = 640
//! height = 360
//! args = ["--schemas", "schemas"] # before every case's own
//!
//! [tolerance]                     # the defaults; a case may override any
//! level = 48                      # a pixel "differs" past this many levels (0-255, max channel)
//! share = 0.02                    # ...and the case fails when more than this share of pixels do
//! mean = 3.0                      # ...or when the mean difference passes this
//!
//! [[case]]
//! name = "ascii-full-pbr"
//! scene = "demo/pbr_test.scene.toml"
//! golden = "ascii_pbr_test.png"   # beside the manifest; default "<name>.png"
//! args = ["--style", "ascii"]
//! ```
//!
//! Each case's render, and a diff image when it fails, land in `--out-dir`
//! (default: the system temp dir), with a `report.json` of every case's
//! numbers. `--bless` writes the renders over the goldens after a
//! deliberate change; look at them before committing. Without a GPU
//! adapter every case is skipped and the suite passes, unless
//! `--require-gpu`.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The flags `render-suite` takes for a `.goldens.toml`.
#[derive(clap::Args, Debug, Default)]
pub struct GoldenArgs {
    /// Goldens: write each render over its golden instead of comparing
    #[arg(long)]
    pub bless: bool,

    /// Goldens: run only the cases whose name contains this (repeatable)
    #[arg(long = "case")]
    pub cases: Vec<String>,

    /// Goldens: where renders, diffs and report.json go (default: a
    /// folder in the system temp dir)
    #[arg(long)]
    pub out_dir: Option<String>,

    /// Goldens: fail, rather than skip, when there is no GPU adapter
    #[arg(long)]
    pub require_gpu: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    suite: Suite,
    #[serde(default)]
    tolerance: Tolerance,
    #[serde(rename = "case", default)]
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Suite {
    name: String,
    /// Relative to the manifest's folder.
    #[serde(default)]
    root: Option<String>,
    #[serde(default = "default_width")]
    width: u32,
    #[serde(default = "default_height")]
    height: u32,
    #[serde(default)]
    args: Vec<String>,
}

fn default_width() -> u32 {
    640
}

fn default_height() -> u32 {
    360
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Tolerance {
    pub level: u8,
    pub share: f64,
    pub mean: f64,
}

impl Default for Tolerance {
    fn default() -> Self {
        // The ASCII golden's (#265): GPUs shade a little differently and a
        // cell on a ramp step's border can flip glyph; a broken grid, ramp
        // or colour mode moves far more than this.
        Self {
            level: 48,
            share: 0.02,
            mean: 3.0,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ToleranceOverride {
    level: Option<u8>,
    share: Option<f64>,
    mean: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    scene: String,
    #[serde(default)]
    golden: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    tolerance: ToleranceOverride,
}

/// How one render compares with its golden.
#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct Comparison {
    /// Share of pixels whose largest channel difference passes the level.
    pub far_share: f64,
    /// Mean of every pixel's largest channel difference.
    pub mean: f64,
    /// The largest difference anywhere.
    pub max: u8,
    /// Pixels that differ at all.
    pub changed: usize,
}

impl Comparison {
    pub fn passes(&self, t: &Tolerance) -> bool {
        self.far_share <= t.share && self.mean <= t.mean
    }
}

/// Compare `got` with `want` (same size) and draw a diff: the golden
/// dimmed to a quarter, amber where a pixel moved, red past the level.
pub fn compare(
    want: &image::RgbaImage,
    got: &image::RgbaImage,
    level: u8,
) -> (Comparison, image::RgbaImage) {
    let (w, h) = want.dimensions();
    let mut diff = image::RgbaImage::new(w, h);
    let mut c = Comparison::default();
    let mut total = 0u64;
    let mut far = 0usize;
    for ((a, b), out) in want.pixels().zip(got.pixels()).zip(diff.pixels_mut()) {
        let d = (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap();
        total += d as u64;
        c.max = c.max.max(d);
        let grey = ((a[0] as u32 * 54 + a[1] as u32 * 183 + a[2] as u32 * 19) >> 10) as u8;
        *out = if d > level {
            far += 1;
            image::Rgba([255, 48, 48, 255])
        } else if d > 0 {
            let k = 96 + (d as u32 * 159 / level.max(1) as u32).min(159);
            image::Rgba([k as u8, (k * 3 / 4) as u8, 32, 255])
        } else {
            image::Rgba([grey, grey, grey, 255])
        };
        c.changed += (d > 0) as usize;
    }
    let pixels = (w * h).max(1) as f64;
    c.far_share = far as f64 / pixels;
    c.mean = total as f64 / pixels;
    (c, diff)
}

#[derive(Serialize)]
struct CaseReport {
    name: String,
    scene: String,
    args: Vec<String>,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    comparison: Option<Comparison>,
    tolerance: Tolerance,
    render: String,
    golden: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

pub fn run(manifest_path: &str, args: &GoldenArgs) -> Result<()> {
    let manifest_path = Path::new(manifest_path);
    let text = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: Manifest =
        toml::from_str(&text).with_context(|| format!("parsing {}", manifest_path.display()))?;
    let here = manifest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let root = match &manifest.suite.root {
        Some(r) => here.join(r),
        None => PathBuf::from("."),
    };
    let out_dir = match &args.out_dir {
        Some(d) => PathBuf::from(d),
        None => std::env::temp_dir()
            .join("flint-goldens")
            .join(&manifest.suite.name),
    };
    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    // `flint render` runs from the suite's root; give it absolute outputs
    let out_dir = std::path::absolute(&out_dir).unwrap_or(out_dir);

    let mut names = std::collections::HashSet::new();
    for case in &manifest.cases {
        if !names.insert(case.name.as_str()) {
            bail!(
                "case '{}' appears twice in {}",
                case.name,
                manifest_path.display()
            );
        }
    }
    // A filter that names a case exactly picks that one; otherwise it
    // picks every case whose name contains it
    let picks = |c: &Case| {
        args.cases.iter().any(|f| {
            let exact = manifest.cases.iter().any(|k| k.name == *f);
            if exact {
                c.name == *f
            } else {
                c.name.contains(f.as_str())
            }
        })
    };
    let chosen: Vec<&Case> = manifest
        .cases
        .iter()
        .filter(|c| args.cases.is_empty() || picks(c))
        .collect();
    if chosen.is_empty() {
        bail!("no cases to run in {}", manifest_path.display());
    }

    let flint = std::env::current_exe().context("finding the flint binary")?;
    println!(
        "render-suite '{}': {} case(s){} from {}",
        manifest.suite.name,
        chosen.len(),
        if args.bless { ", blessing" } else { "" },
        root.display()
    );

    let mut reports = Vec::new();
    let mut failed = 0usize;
    let mut no_gpu = false;
    for case in chosen {
        let width = case.width.unwrap_or(manifest.suite.width);
        let height = case.height.unwrap_or(manifest.suite.height);
        let tolerance = Tolerance {
            level: case.tolerance.level.unwrap_or(manifest.tolerance.level),
            share: case.tolerance.share.unwrap_or(manifest.tolerance.share),
            mean: case.tolerance.mean.unwrap_or(manifest.tolerance.mean),
        };
        let golden = here.join(
            case.golden
                .clone()
                .unwrap_or_else(|| format!("{}.png", case.name)),
        );
        let render = out_dir.join(format!("{}.png", case.name));
        let mut report = CaseReport {
            name: case.name.clone(),
            scene: case.scene.clone(),
            args: case.args.clone(),
            status: "skip",
            comparison: None,
            tolerance,
            render: render.display().to_string(),
            golden: golden.display().to_string(),
            diff: None,
            message: None,
        };
        if no_gpu {
            report.message = Some("no GPU adapter".into());
            reports.push(report);
            continue;
        }

        let _ = std::fs::remove_file(&render);
        let started = std::time::Instant::now();
        let output = Command::new(&flint)
            .current_dir(&root)
            .arg("render")
            .arg(&case.scene)
            .args([
                "--width",
                &width.to_string(),
                "--height",
                &height.to_string(),
            ])
            .args(&manifest.suite.args)
            .args(&case.args)
            .arg("--output")
            .arg(&render)
            .output()
            .with_context(|| format!("running {}", flint.display()))?;
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let ms = started.elapsed().as_millis();
        if !output.status.success() {
            if log.to_lowercase().contains("adapter") {
                no_gpu = true;
                println!("  SKIP  {:<28} no GPU adapter", case.name);
                report.message = Some("no GPU adapter".into());
                reports.push(report);
                continue;
            }
            failed += 1;
            println!("  FAIL  {:<28} flint render failed:\n{log}", case.name);
            report.status = "error";
            report.message = Some(log);
            reports.push(report);
            continue;
        }
        let got = image::open(&render)
            .with_context(|| format!("reading {}", render.display()))?
            .to_rgba8();

        if args.bless {
            if let Some(parent) = golden.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            got.save(&golden)
                .with_context(|| format!("writing {}", golden.display()))?;
            println!("  BLESS {:<28} -> {}", case.name, golden.display());
            report.status = "blessed";
            reports.push(report);
            continue;
        }

        let want = match image::open(&golden) {
            Ok(img) => img.to_rgba8(),
            Err(e) => {
                failed += 1;
                println!(
                    "  FAIL  {:<28} no golden at {} ({e}); --bless makes it",
                    case.name,
                    golden.display()
                );
                report.status = "missing";
                report.message = Some(format!("no golden: {e}"));
                reports.push(report);
                continue;
            }
        };
        if want.dimensions() != got.dimensions() {
            failed += 1;
            println!(
                "  FAIL  {:<28} size {:?}, golden {:?}",
                case.name,
                got.dimensions(),
                want.dimensions()
            );
            report.status = "fail";
            report.message = Some("size differs from the golden".into());
            reports.push(report);
            continue;
        }
        let (cmp, diff) = compare(&want, &got, tolerance.level);
        let pass = cmp.passes(&tolerance);
        println!(
            "  {}  {:<28} {:>6.2}% past {} (limit {:.2}%), mean {:.2} (limit {:.2}), max {}, {} ms",
            if pass { "PASS" } else { "FAIL" },
            case.name,
            cmp.far_share * 100.0,
            tolerance.level,
            tolerance.share * 100.0,
            cmp.mean,
            tolerance.mean,
            cmp.max,
            ms
        );
        if !pass || cmp.changed > 0 {
            let path = out_dir.join(format!("{}.diff.png", case.name));
            diff.save(&path).ok();
            report.diff = Some(path.display().to_string());
        }
        report.status = if pass { "pass" } else { "fail" };
        report.comparison = Some(cmp);
        failed += (!pass) as usize;
        reports.push(report);
    }

    let report_path = out_dir.join("report.json");
    std::fs::write(&report_path, serde_json::to_string_pretty(&reports)?)
        .with_context(|| format!("writing {}", report_path.display()))?;
    println!("renders and report in {}", out_dir.display());

    if no_gpu {
        if args.require_gpu {
            bail!("no GPU adapter, and --require-gpu");
        }
        println!("no GPU adapter: skipped (pass --require-gpu to fail instead)");
        return Ok(());
    }
    if failed > 0 {
        bail!("{failed} case(s) failed against their goldens");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(v: u8) -> image::RgbaImage {
        image::RgbaImage::from_pixel(10, 10, image::Rgba([v, v, v, 255]))
    }

    #[test]
    fn identical_images_compare_clean() {
        let (c, _) = compare(&solid(100), &solid(100), 48);
        assert_eq!(c, Comparison::default());
        assert!(c.passes(&Tolerance::default()));
    }

    #[test]
    fn a_few_far_pixels_pass_and_many_fail() {
        let want = solid(0);
        let mut got = solid(0);
        got.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        let (c, diff) = compare(&want, &got, 48);
        assert_eq!((c.max, c.changed), (255, 1));
        assert!((c.far_share - 0.01).abs() < 1e-9);
        assert!(c.passes(&Tolerance::default()));
        assert_eq!(diff.get_pixel(0, 0)[0], 255);

        let (c, _) = compare(&want, &solid(200), 48);
        assert!(!c.passes(&Tolerance::default()));
    }

    fn stylize_manifest() -> (PathBuf, Manifest) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/stylize.goldens.toml");
        let manifest = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        (path, manifest)
    }

    #[test]
    fn every_case_has_a_golden_and_every_golden_a_case() {
        let (path, manifest) = stylize_manifest();
        let dir = path.parent().unwrap();
        let mut used = std::collections::HashSet::new();
        for case in &manifest.cases {
            let name = case
                .golden
                .clone()
                .unwrap_or_else(|| format!("{}.png", case.name));
            assert!(dir.join(&name).exists(), "{} has no golden", case.name);
            used.insert(name);
        }
        for entry in std::fs::read_dir(dir).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            if name.ends_with(".png") {
                assert!(
                    used.contains(&name),
                    "{name} is no case's golden: remove it, or add its case"
                );
            }
        }
    }

    /// A new style adds a golden case (#271): every registered style is
    /// reached by some case's `--style <name>` (`mask_view` by the style
    /// debug view that turns it on).
    #[test]
    fn every_registered_style_has_a_golden_case() {
        let (_, manifest) = stylize_manifest();
        for d in flint_render::stylize::registry() {
            let reached = manifest.cases.iter().any(|c| {
                c.args.windows(2).any(|w| {
                    (w[0] == "--style"
                        && (w[1] == d.name || w[1].starts_with(&format!("{}:", d.name))))
                        || (d.name == "mask_view" && w[0] == "--debug-mode" && w[1] == "style")
                })
            });
            assert!(
                reached,
                "style '{}' has no case in stylize.goldens.toml: add one and --bless it",
                d.name
            );
        }
    }

    /// A new look adds a golden case too (#279).
    #[test]
    fn every_shipped_look_has_a_golden_case() {
        let (_, manifest) = stylize_manifest();
        for look in crate::commands::look::builtin_names() {
            let reached = manifest.cases.iter().any(|c| {
                c.args
                    .windows(2)
                    .any(|w| w[0] == "--look" && w[1] == look)
            });
            assert!(
                reached,
                "look '{look}' has no case in stylize.goldens.toml: add one and --bless it"
            );
        }
    }
}
