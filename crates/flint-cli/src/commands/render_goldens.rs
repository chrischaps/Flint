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
//! `--require-gpu` or `FLINT_REQUIRE_GPU=1` (#314; CI's software-adapter
//! job sets the variable).
//!
//! Adapters differ (#314): a golden may have a variant per adapter beside
//! it, `<stem>.<key>.png`, where the key is [`adapter_key`]'s (`warp`,
//! `lavapipe`, else the backend: `vulkan`, `dx12`, `metal`, `gl`). A case
//! compares with its adapter's variant when there is one, else with the
//! plain golden. `--bless` rewrites whichever of the two the case compares
//! with; `--bless-variant` writes this adapter's variant. Each report
//! names the adapter, the golden compared and where a variant would go.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The flags `render-suite` takes for a `.goldens.toml`.
#[derive(clap::Args, Debug, Default)]
pub struct GoldenArgs {
    /// Goldens: write each render over its golden instead of comparing
    /// (this adapter's variant, when the case has one)
    #[arg(long)]
    pub bless: bool,

    /// Goldens: write each render as this adapter's variant of its golden,
    /// `<stem>.<adapter>.png`, for a case another adapter draws differently
    #[arg(long)]
    pub bless_variant: bool,

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
    /// The golden this case compared with: the plain one or a variant.
    golden: String,
    /// Where this adapter's variant of the golden goes (#314), whether or
    /// not there is one: the path to copy a render to when blessing a
    /// variant by hand from another machine (a CI artifact).
    #[serde(skip_serializing_if = "Option::is_none")]
    variant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    adapter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

/// The key an adapter's golden variants carry (#314): `warp` for the
/// software DX12 adapter, `lavapipe` for Mesa's software Vulkan, else the
/// backend's name. Keyed by backend rather than by GPU: one card under
/// DX12 and Vulkan differs more than two cards under one backend.
pub fn adapter_key(info: &wgpu::AdapterInfo) -> String {
    let software = info.device_type == wgpu::DeviceType::Cpu
        || info.name.contains("Basic Render Driver")
        || info.name.to_lowercase().contains("llvmpipe");
    match info.backend {
        wgpu::Backend::Dx12 if software => "warp".into(),
        wgpu::Backend::Vulkan if software => "lavapipe".into(),
        b => format!("{b:?}").to_lowercase(),
    }
}

/// `<dir>/<stem>.<key>.png`: the variant of `golden` for adapter `key`.
fn variant_path(golden: &Path, key: &str) -> PathBuf {
    let stem = golden
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    golden.with_file_name(format!("{stem}.{key}.png"))
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
    // The adapter the cases' `flint render`s will pick (they inherit
    // WGPU_BACKEND / WGPU_ADAPTER_NAME), so a report says what drew it:
    // the tolerance holds between adapters, not pixels (#314)
    let info = pollster::block_on(flint_render::gpu_select::request_adapter(
        &flint_render::gpu_select::instance(),
        None,
    ))
    .map(|a| a.get_info());
    let adapter = info
        .as_ref()
        .map(|i| format!("{} ({:?}, {:?})", i.name, i.device_type, i.backend));
    let key = info.as_ref().map(adapter_key);
    println!(
        "adapter: {}{}",
        adapter.as_deref().unwrap_or("none found"),
        key.as_deref()
            .map(|k| format!(", golden variants '*.{k}.png'"))
            .unwrap_or_default()
    );
    if args.bless_variant && key.is_none() {
        bail!("--bless-variant needs an adapter to name the variant after");
    }

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
        let plain = here.join(
            case.golden
                .clone()
                .unwrap_or_else(|| format!("{}.png", case.name)),
        );
        // This adapter's variant, when it has one, is the golden (#314)
        let variant = key.as_deref().map(|k| variant_path(&plain, k));
        let golden = match &variant {
            Some(v) if args.bless_variant || v.exists() => v.clone(),
            _ => plain.clone(),
        };
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
            variant: variant.as_ref().map(|v| v.display().to_string()),
            adapter: adapter.clone(),
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

        if args.bless || args.bless_variant {
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
            "  {}  {:<28} {:>6.2}% past {} (limit {:.2}%), mean {:.2} (limit {:.2}), max {}, {} ms{}",
            if pass { "PASS" } else { "FAIL" },
            case.name,
            cmp.far_share * 100.0,
            tolerance.level,
            tolerance.share * 100.0,
            cmp.mean,
            tolerance.mean,
            cmp.max,
            ms,
            if golden != plain { ", against its variant" } else { "" }
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
        if args.require_gpu || flint_render::gpu_select::gpu_required() {
            bail!(
                "no GPU adapter, and --require-gpu (or FLINT_REQUIRE_GPU); adapters: {:?}",
                flint_render::list_adapters()
            );
        }
        println!("no GPU adapter: skipped (--require-gpu or FLINT_REQUIRE_GPU=1 fails instead)");
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

    #[test]
    fn variants_are_keyed_by_backend_and_software_adapters_by_name() {
        let info = |name: &str, device_type, backend| wgpu::AdapterInfo {
            name: name.into(),
            vendor: 0,
            device: 0,
            device_type,
            driver: String::new(),
            driver_info: String::new(),
            backend,
        };
        use wgpu::{Backend, DeviceType};
        let warp = info(
            "Microsoft Basic Render Driver",
            DeviceType::Cpu,
            Backend::Dx12,
        );
        assert_eq!(adapter_key(&warp), "warp");
        // Headless Windows lists a WARP that claims to be integrated
        let warp2 = info(
            "Microsoft Basic Render Driver",
            DeviceType::IntegratedGpu,
            Backend::Dx12,
        );
        assert_eq!(adapter_key(&warp2), "warp");
        let lava = info(
            "llvmpipe (LLVM 15.0.7, 256 bits)",
            DeviceType::Cpu,
            Backend::Vulkan,
        );
        assert_eq!(adapter_key(&lava), "lavapipe");
        let rtx = info(
            "NVIDIA GeForce RTX 4070",
            DeviceType::DiscreteGpu,
            Backend::Vulkan,
        );
        assert_eq!(adapter_key(&rtx), "vulkan");
        let rtx = info(
            "NVIDIA GeForce RTX 4070",
            DeviceType::DiscreteGpu,
            Backend::Dx12,
        );
        assert_eq!(adapter_key(&rtx), "dx12");
        assert_eq!(
            variant_path(Path::new("g/kuwahara.png"), "warp"),
            Path::new("g/kuwahara.warp.png")
        );
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
        // An adapter's variant (#314) belongs to the golden it varies
        let keys = ["warp", "lavapipe", "vulkan", "dx12", "metal", "gl"];
        for entry in std::fs::read_dir(dir).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".png") else {
                continue;
            };
            let owner = match stem.rsplit_once('.') {
                Some((base, key)) => {
                    assert!(keys.contains(&key), "{name}: unknown adapter key '{key}'");
                    format!("{base}.png")
                }
                None => name.clone(),
            };
            assert!(
                used.contains(&owner),
                "{name} is no case's golden: remove it, or add its case"
            );
        }
    }

    /// A new style adds a golden case (#271): every registered style is
    /// reached by some case's `--style <name>` (`mask_view` by the style
    /// debug view that turns it on).
    #[test]
    fn every_registered_style_has_a_golden_case() {
        let (_, manifest) = stylize_manifest();
        // Built-ins only: user styles (ADR 0112) that other tests in this
        // process registered aren't the engine's to golden (#417)
        for d in flint_render::stylize::registry()
            .iter()
            .filter(|d| flint_render::stylize::is_builtin(d.name))
        {
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
