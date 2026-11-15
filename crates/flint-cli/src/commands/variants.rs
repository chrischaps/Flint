//! `flint render --variant` / `--variants` (#291): the same scene, at the same
//! camera, under several sets of flags, as one labelled sheet, with
//! nearest-neighbour zooms of a region of every variant (`--zoom`).
//!
//! ```text
//! flint render examples/ascii_entities.scene.toml -o cmp.png \
//!     --variant "as authored:" \
//!     --variant "full screen: --ascii-scope screen" \
//!     --variant "object grid: --ascii-grid object" \
//!     --zoom 800,380,1120,560@3
//! ```
//!
//! A `--sweep` turns one key; a variant is any set of `flint render` flags, so
//! two looks, a look against a style, MSAA on and off, or three palettes at two
//! cell sizes can sit side by side. Each variant is the base command line (every
//! flag but these three and `--output`) with its own flags after it, parsed by
//! the same `RenderArgs` the subcommand takes. A flag given in both is the
//! variant's (clap's `args_override_self`), and a repeatable one (`--style`,
//! `--sweep`) adds to the base's.
//!
//! Every variant renders in this process on one GPU device (the
//! [`RenderHost`] `flint goldens` shares between cases), with glTF decodes and
//! mip chains memoized, so a sheet of N is one process start and one device,
//! not N. Each still builds its own `SceneRenderer`, since a variant may change
//! anything a renderer is built with (MSAA, shadows, the scene's styles).

use super::render::{RenderArgs, RenderHost};
use super::style_sheet::{self, SheetText};
use anyhow::{anyhow, bail, Context, Result};
use image::{Rgba, RgbaImage};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Most variants on a sheet: four rows of four.
pub const MAX_VARIANTS: usize = 16;

/// Frames wider than this are shrunk on the sheet (the zooms and each
/// variant's own PNG keep every pixel).
pub const SHEET_CELL_WIDTH: u32 = 960;

/// The colour a zoom's region is outlined in on the sheet.
const LOCATOR: Rgba<u8> = style_sheet::TITLE;

/// One variant: a label and the flags it adds to the base command.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub label: String,
    pub flags: Vec<String>,
}

impl Variant {
    /// The flags as one line, for the sheet and the terminal.
    pub fn flags_text(&self) -> String {
        if self.flags.is_empty() {
            "(the base command)".into()
        } else {
            self.flags
                .iter()
                .map(|f| quote(f))
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

/// `clap`'s value parser for `--variant "label: --flag value ..."`.
///
/// The label is everything before the first `:`, unless the text starts
/// with `-`, when it's all flags and labelled with them. `"label:"` or a
/// bare `"label"` is the base command as given.
pub fn parse_variant(spec: &str) -> Result<Variant, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("--variant is empty: give \"label: --flag value ...\"".into());
    }
    let (label, rest) = if spec.starts_with('-') {
        (None, spec)
    } else {
        match spec.split_once(':') {
            Some((label, rest)) => (Some(label.trim()), rest),
            None => (Some(spec), ""),
        }
    };
    let flags = split_words(rest).map_err(|e| format!("--variant '{spec}': {e}"))?;
    if let Some(word) = flags.first().filter(|w| !w.starts_with('-')) {
        return Err(format!(
            "--variant '{spec}': '{word}' isn't a flag; a variant's words are flags \
             for the same scene (write \"label: --flag value\")"
        ));
    }
    let label = match label {
        Some(l) if !l.is_empty() => l.to_string(),
        _ => rest.trim().to_string(),
    };
    Ok(Variant { label, flags })
}

/// Split a line of flags into words: whitespace separates, and `'...'` or
/// `"..."` keeps one word together (`\"` and `\\` inside double quotes).
/// A backslash anywhere else is a backslash, so Windows paths survive.
pub fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("a ' is never closed".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') if matches!(chars.peek(), Some('"' | '\\')) => {
                            word.push(chars.next().unwrap());
                        }
                        Some(c) => word.push(c),
                        None => return Err("a \" is never closed".into()),
                    }
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// A word as it would be typed: quoted when it holds a space or a quote.
fn quote(word: &str) -> String {
    if word.is_empty() || word.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'') {
        format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        word.to_string()
    }
}

/// `--zoom X0,Y0,X1,Y1[@N]`: a region of the frame, in its pixels, drawn
/// N times larger with nearest-neighbour so every pixel stays a pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    /// `@N`; without it, as large as fits 640 pixels across (at most 8).
    pub scale: Option<u32>,
}

/// `clap`'s value parser for `--zoom`.
pub fn parse_zoom(spec: &str) -> Result<Zoom, String> {
    let usage = || {
        format!(
            "--zoom takes X0,Y0,X1,Y1[@N] in pixels of the frame (got '{spec}'), \
             e.g. 800,380,1120,560@3"
        )
    };
    let (region, scale) = match spec.split_once('@') {
        Some((region, n)) => {
            let n: u32 = n.trim().parse().map_err(|_| usage())?;
            if !(1..=16).contains(&n) {
                return Err(format!("--zoom {spec}: @{n} is out of range (1 to 16)"));
            }
            (region, Some(n))
        }
        None => (spec, None),
    };
    let numbers: Option<Vec<u32>> = region.split(',').map(|p| p.trim().parse().ok()).collect();
    let Some(&[x0, y0, x1, y1]) = numbers.as_deref() else {
        return Err(usage());
    };
    if x1 <= x0 || y1 <= y0 {
        return Err(format!(
            "--zoom {spec}: X1,Y1 is the far corner, so it must be right of and below X0,Y0"
        ));
    }
    Ok(Zoom {
        x0,
        y0,
        x1,
        y1,
        scale,
    })
}

impl Zoom {
    pub fn width(&self) -> u32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> u32 {
        self.y1 - self.y0
    }

    /// How many times larger the crop is drawn.
    pub fn scale(&self) -> u32 {
        self.scale
            .unwrap_or_else(|| (640 / self.width()).clamp(1, 8))
    }

    /// A zoom that leaves the frame is refused, naming the frame.
    pub fn check(&self, width: u32, height: u32) -> Result<()> {
        if self.x1 > width || self.y1 > height {
            bail!(
                "--zoom {}: reaches past the {width}x{height} frame",
                self.describe()
            );
        }
        Ok(())
    }

    /// The region, nearest-neighbour scaled.
    pub fn crop(&self, frame: &RgbaImage) -> RgbaImage {
        let region =
            image::imageops::crop_imm(frame, self.x0, self.y0, self.width(), self.height())
                .to_image();
        let s = self.scale();
        if s == 1 {
            return region;
        }
        image::imageops::resize(
            &region,
            self.width() * s,
            self.height() * s,
            image::imageops::FilterType::Nearest,
        )
    }

    /// `800,380,1120,560@3`.
    pub fn describe(&self) -> String {
        format!(
            "{},{},{},{}@{}",
            self.x0,
            self.y0,
            self.x1,
            self.y1,
            self.scale()
        )
    }
}

/// A `--variants` file.
///
/// ```toml
/// title = "ASCII: full screen against per entity"   # optional
/// zoom = ["800,380,1120,560@3"]                    # optional, as --zoom
///
/// [[variant]]
/// label = "full screen"
/// flags = "--ascii-scope screen"                    # or ["--ascii-scope", "screen"]
/// ```
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct VariantsFile {
    title: Option<String>,
    #[serde(default)]
    zoom: Vec<String>,
    #[serde(default, rename = "variant")]
    variants: Vec<FileVariant>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FileVariant {
    label: String,
    #[serde(default)]
    flags: Flags,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(untagged)]
enum Flags {
    #[default]
    None,
    Line(String),
    Words(Vec<String>),
}

/// What a `--variants` file holds.
#[derive(Debug, Default)]
pub struct Loaded {
    pub title: Option<String>,
    pub zooms: Vec<Zoom>,
    pub variants: Vec<Variant>,
}

/// Read a `--variants` file.
pub fn load_file(path: &Path) -> Result<Loaded> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_file(&text).with_context(|| format!("--variants {}", path.display()))
}

fn parse_file(text: &str) -> Result<Loaded> {
    let file: VariantsFile = toml::from_str(text)?;
    let mut variants = Vec::new();
    for (i, v) in file.variants.into_iter().enumerate() {
        let flags = match v.flags {
            Flags::None => Vec::new(),
            Flags::Line(line) => {
                split_words(&line).map_err(|e| anyhow!("variant {}: {e}", i + 1))?
            }
            Flags::Words(words) => words,
        };
        if let Some(word) = flags.first().filter(|w| !w.starts_with('-')) {
            bail!(
                "variant '{}': '{word}' isn't a flag; a variant's words are flags for the \
                 same scene",
                v.label
            );
        }
        if v.label.trim().is_empty() {
            bail!("variant {} has an empty label", i + 1);
        }
        variants.push(Variant {
            label: v.label.trim().to_string(),
            flags,
        });
    }
    let zooms = file
        .zoom
        .iter()
        .map(|z| parse_zoom(z).map_err(|e| anyhow!(e)))
        .collect::<Result<_>>()?;
    Ok(Loaded {
        title: file.title,
        zooms,
        variants,
    })
}

/// The flags that shape the variants themselves, and `--output`: none of
/// them go into a variant's own command line.
const OWN_FLAGS: [&str; 4] = ["--variant", "--variants", "--zoom", "--output"];

/// `flint render`'s words with this command's own flags taken out: what
/// each variant's flags go after.
pub fn base_words(words: &[OsString]) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let word = words[i].to_string_lossy();
        let takes_next = OWN_FLAGS.contains(&word.as_ref()) || word == "-o";
        let joined = OWN_FLAGS.iter().any(|f| word.starts_with(&format!("{f}=")))
            || (word.starts_with("-o") && !word.starts_with("--"));
        if takes_next {
            i += 2;
        } else if joined {
            i += 1;
        } else {
            out.push(words[i].clone());
            i += 1;
        }
    }
    out
}

/// The words after `render` in this process's command line, without the
/// global `--project` (already applied): what `--variant` builds on.
pub fn subcommand_words(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut words = args.into_iter().skip(1);
    let mut tail = Vec::new();
    let mut found = false;
    while let Some(word) = words.next() {
        let text = word.to_string_lossy();
        if text == "--project" {
            words.next();
            continue;
        }
        if text.starts_with("--project=") {
            continue;
        }
        if found {
            tail.push(word);
        } else if !text.starts_with('-') {
            found = true;
        }
    }
    tail
}

/// Parse one variant's command line as `flint render` would, a flag given
/// twice taking its last value.
fn parse_render(words: Vec<OsString>) -> Result<RenderArgs> {
    use clap::{Args, FromArgMatches};
    let command =
        RenderArgs::augment_args(clap::Command::new("flint render")).args_override_self(true);
    let matches = command
        .try_get_matches_from(std::iter::once(OsString::from("flint render")).chain(words))
        .map_err(|e| anyhow!("{e}"))?;
    RenderArgs::from_arg_matches(&matches).map_err(|e| anyhow!("{e}"))
}

/// A file name's worth of a label: `full screen` -> `full-screen`.
pub fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "variant".into()
    } else {
        out
    }
}

/// One slug a variant, each unique (`-2`, `-3` after a repeat).
fn slugs(variants: &[Variant]) -> Vec<String> {
    let mut seen = std::collections::HashMap::<String, usize>::new();
    variants
        .iter()
        .map(|v| {
            let base = slug(&v.label);
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            if *n == 1 {
                base
            } else {
                format!("{base}-{n}")
            }
        })
        .collect()
}

/// Where everything is written, from `--output`.
struct Outputs {
    dir: PathBuf,
    stem: String,
}

impl Outputs {
    fn new(output: &str) -> Result<Self> {
        let path = Path::new(output);
        let is_png = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"));
        if !is_png {
            bail!("--variant writes a PNG sheet: give -o <name>.png (got '{output}')");
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "render".into());
        let dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(Self { dir, stem })
    }

    fn file(&self, suffix: &str) -> PathBuf {
        self.dir.join(format!("{}-{suffix}.png", self.stem))
    }

    /// A zoom's suffix: `zoom`, or `zoom2`, `zoom3` after the first.
    fn zoom_suffix(k: usize) -> String {
        if k == 0 {
            "zoom".into()
        } else {
            format!("zoom{}", k + 1)
        }
    }
}

/// The labelled sheet of every variant, and a sheet of each zoom.
pub fn run(args: &RenderArgs, host: &mut RenderHost) -> Result<()> {
    let mut loaded = match &args.variants {
        Some(path) => load_file(Path::new(path))?,
        None => Loaded::default(),
    };
    loaded.variants.extend(args.variant.iter().cloned());
    loaded.zooms.extend(args.zoom.iter().copied());
    let Loaded {
        title,
        zooms,
        variants,
    } = loaded;
    if variants.is_empty() {
        bail!(
            "--variants {}: the file lists no [[variant]]",
            args.variants.as_deref().unwrap_or("")
        );
    }
    if variants.len() > MAX_VARIANTS {
        bail!(
            "{} variants: a sheet takes at most {MAX_VARIANTS}",
            variants.len()
        );
    }
    if args.command_line.is_empty() {
        bail!("--variant needs flint render's own command line, which this caller didn't pass");
    }
    let base = base_words(&args.command_line);
    let outputs = Outputs::new(&args.output)?;
    std::fs::create_dir_all(&outputs.dir)
        .with_context(|| format!("creating {}", outputs.dir.display()))?;
    let slugs = slugs(&variants);

    // Parse every variant before any draws, so a typo in the last costs nothing
    let mut parsed = Vec::with_capacity(variants.len());
    for (variant, slug) in variants.iter().zip(&slugs) {
        let mut words = base.clone();
        words.extend(variant.flags.iter().map(OsString::from));
        let path = outputs.file(slug);
        words.push("--output".into());
        words.push(path.clone().into_os_string());
        let mut render = parse_render(words)
            .with_context(|| format!("variant '{}' ({})", variant.label, variant.flags_text()))?;
        if render.variants.is_some() || !render.variant.is_empty() || !render.zoom.is_empty() {
            bail!(
                "variant '{}': a variant's flags can't hold --variant, --variants or --zoom",
                variant.label
            );
        }
        render.command_line = Vec::new();
        parsed.push((render, path));
    }

    flint_import::memoize_imports(true);
    flint_render::memoize_mip_chains(true);
    let drawn = draw_all(&variants, parsed, host);
    flint_import::memoize_imports(false);
    flint_render::memoize_mip_chains(false);
    let frames = drawn?;

    let (width, height) = frames[0].dimensions();
    for (frame, variant) in frames.iter().zip(&variants).skip(1) {
        if frame.dimensions() != (width, height) {
            let (w, h) = frame.dimensions();
            bail!(
                "variant '{}' drew {w}x{h} but '{}' drew {width}x{height}: a sheet's \
                 variants share one size (drop the --width/--height/--sweep that differs)",
                variant.label,
                variants[0].label
            );
        }
    }
    for zoom in &zooms {
        zoom.check(width, height)?;
    }
    for i in 0..frames.len().saturating_sub(1) {
        if frames[i] == frames[i + 1] {
            eprintln!(
                "Warning: variants '{}' and '{}' are pixel-identical: the flags that differ \
                 change nothing this frame shows",
                variants[i].label,
                variants[i + 1].label
            );
        }
    }

    let scene_name = title.clone().unwrap_or_else(|| {
        Path::new(&args.scene)
            .file_name()
            .map(|s| {
                s.to_string_lossy()
                    .trim_end_matches(".scene.toml")
                    .to_string()
            })
            .unwrap_or_default()
    });
    let labels: Vec<String> = variants.iter().map(|v| v.label.clone()).collect();
    let notes: Vec<String> = variants.iter().map(Variant::flags_text).collect();

    // The sheet: each frame (shrunk past SHEET_CELL_WIDTH), each zoom's
    // region outlined
    let shrink = |frame: &RgbaImage| -> (RgbaImage, f64) {
        if width <= SHEET_CELL_WIDTH {
            return (frame.clone(), 1.0);
        }
        let h = ((u64::from(height) * u64::from(SHEET_CELL_WIDTH) + u64::from(width) / 2)
            / u64::from(width))
        .max(1) as u32;
        let small = image::imageops::resize(
            frame,
            SHEET_CELL_WIDTH,
            h,
            image::imageops::FilterType::Triangle,
        );
        (small, f64::from(SHEET_CELL_WIDTH) / f64::from(width))
    };
    let cells: Vec<RgbaImage> = frames
        .iter()
        .map(|frame| {
            let (mut cell, k) = shrink(frame);
            for zoom in &zooms {
                outline(&mut cell, zoom, k);
            }
            cell
        })
        .collect();
    let shown = if width > SHEET_CELL_WIDTH {
        format!(
            "  {width}x{height} shown at {}%",
            (SHEET_CELL_WIDTH * 100 + width / 2) / width
        )
    } else {
        format!("  {width}x{height}")
    };
    let zoom_note = if zooms.is_empty() {
        String::new()
    } else {
        format!(
            "  zoom {}",
            zooms
                .iter()
                .map(Zoom::describe)
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    let mut text = SheetText::new(
        scene_name.clone(),
        format!("{} variants{shown}{zoom_note}", variants.len()),
        labels.clone(),
    );
    text.notes = notes.clone();
    let sheet = style_sheet::compose(&cells, &text);
    sheet
        .save(&args.output)
        .with_context(|| format!("writing {}", args.output))?;
    println!(
        "Wrote {} variants as one {}x{} sheet to {} (each variant's frame beside it as {})",
        variants.len(),
        sheet.width(),
        sheet.height(),
        args.output,
        outputs.file("<label>").display()
    );

    for (k, zoom) in zooms.iter().enumerate() {
        let suffix = Outputs::zoom_suffix(k);
        let crops: Vec<RgbaImage> = frames.iter().map(|f| zoom.crop(f)).collect();
        for (crop, slug) in crops.iter().zip(&slugs) {
            let path = outputs.file(&format!("{slug}-{suffix}"));
            crop.save(&path)
                .with_context(|| format!("writing {}", path.display()))?;
        }
        let mut text = SheetText::new(
            scene_name.clone(),
            format!(
                "zoom {},{} to {},{} of {width}x{height}  {}x nearest",
                zoom.x0,
                zoom.y0,
                zoom.x1,
                zoom.y1,
                zoom.scale()
            ),
            labels.clone(),
        );
        text.notes = notes.clone();
        let sheet = style_sheet::compose(&crops, &text);
        let path = outputs.file(&suffix);
        sheet
            .save(&path)
            .with_context(|| format!("writing {}", path.display()))?;
        println!(
            "Wrote zoom {} as a {}x{} sheet to {}",
            zoom.describe(),
            sheet.width(),
            sheet.height(),
            path.display()
        );
    }
    Ok(())
}

/// Render each variant on `host`'s GPU and read its frame back.
fn draw_all(
    variants: &[Variant],
    parsed: Vec<(RenderArgs, PathBuf)>,
    host: &mut RenderHost,
) -> Result<Vec<RgbaImage>> {
    let n = variants.len();
    let mut frames = Vec::with_capacity(n);
    for (i, ((render, path), variant)) in parsed.into_iter().zip(variants).enumerate() {
        println!(
            "[{}/{n}] {}: {}",
            i + 1,
            variant.label,
            variant.flags_text()
        );
        // A stale file from an earlier run must not pass for this one's frame
        let _ = std::fs::remove_file(&path);
        super::render::run_in(render, host)
            .with_context(|| format!("variant '{}'", variant.label))?;
        let frame = image::open(&path)
            .map_err(|e| {
                anyhow!(
                    "variant '{}' wrote no single frame to {} ({e}): a variant is a still, \
                     so drop --frames, --mesh or a --time span from it",
                    variant.label,
                    path.display()
                )
            })?
            .to_rgba8();
        frames.push(frame);
    }
    Ok(frames)
}

/// Outline a zoom's region on a sheet cell drawn at `k` of the frame's size,
/// a pixel outside the region so the region itself stays as drawn.
fn outline(cell: &mut RgbaImage, zoom: &Zoom, k: f64) {
    let (w, h) = cell.dimensions();
    let at = |v: u32| (f64::from(v) * k).round() as i64;
    let (x0, y0) = (at(zoom.x0) - 1, at(zoom.y0) - 1);
    let (x1, y1) = (at(zoom.x1), at(zoom.y1));
    let mut put = |x: i64, y: i64| {
        if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
            cell.put_pixel(x as u32, y as u32, LOCATOR);
        }
    };
    for x in x0..=x1 {
        put(x, y0);
        put(x, y1);
    }
    for y in y0..=y1 {
        put(x0, y);
        put(x1, y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<OsString> {
        split_words(line)
            .unwrap()
            .into_iter()
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn a_variant_is_a_label_and_its_flags() {
        let v = parse_variant("full screen: --ascii-scope screen --style ascii:cell=12").unwrap();
        assert_eq!(v.label, "full screen");
        assert_eq!(
            v.flags,
            ["--ascii-scope", "screen", "--style", "ascii:cell=12"]
        );
        // Flags alone label themselves; a bare label is the base command
        let v = parse_variant("--no-shadows").unwrap();
        assert_eq!((v.label.as_str(), v.flags.len()), ("--no-shadows", 1));
        let v = parse_variant("as authored").unwrap();
        assert_eq!((v.label.as_str(), v.flags.len()), ("as authored", 0));
        let v = parse_variant("as authored:").unwrap();
        assert!(v.flags.is_empty());
        assert!(parse_variant("x: examples/other.scene.toml").is_err());
        assert!(parse_variant("  ").is_err());
    }

    #[test]
    fn words_split_on_spaces_outside_quotes_and_keep_backslashes() {
        assert_eq!(
            split_words(
                r#"--ascii-palette palettes\warm.palette.toml --look 'a b' --x "say \"hi\"""#
            )
            .unwrap(),
            [
                "--ascii-palette",
                r"palettes\warm.palette.toml",
                "--look",
                "a b",
                "--x",
                "say \"hi\""
            ]
        );
        assert!(split_words("--look 'open").is_err());
        assert_eq!(quote("a b"), "\"a b\"");
        assert_eq!(quote("ascii:cell=8"), "ascii:cell=8");
    }

    #[test]
    fn zooms_parse_check_and_crop_nearest() {
        let z = parse_zoom("10,20,30,25@3").unwrap();
        assert_eq!((z.width(), z.height(), z.scale()), (20, 5, 3));
        assert_eq!(parse_zoom("0,0,320,180").unwrap().scale(), 2);
        assert_eq!(parse_zoom("0,0,40,40").unwrap().scale(), 8);
        assert!(parse_zoom("30,20,10,25").is_err());
        assert!(parse_zoom("1,2,3").is_err());
        assert!(parse_zoom("0,0,4,4@0").is_err());
        assert!(z.check(30, 25).is_ok());
        assert!(z.check(29, 25).is_err());

        let mut frame = RgbaImage::new(40, 30);
        frame.put_pixel(10, 20, Rgba([255, 0, 0, 255]));
        let crop = z.crop(&frame);
        assert_eq!(crop.dimensions(), (60, 15));
        // The one red pixel is a 3x3 block, every one of them exactly red
        for y in 0..3 {
            for x in 0..3 {
                assert_eq!(crop.get_pixel(x, y), &Rgba([255, 0, 0, 255]));
            }
        }
        assert_eq!(crop.get_pixel(3, 0), &Rgba([0, 0, 0, 0]));
    }

    #[test]
    fn the_base_command_drops_this_commands_own_flags() {
        let base = base_words(&words(
            "examples/a.scene.toml --variant 'x: --ascii' -o out.png --zoom 0,0,9,9 \
             --variants=v.toml --width 640 -oother.png --output=o.png --distance 5",
        ));
        assert_eq!(
            base,
            words("examples/a.scene.toml --width 640 --distance 5")
        );
    }

    #[test]
    fn the_subcommand_words_skip_the_global_project() {
        let argv =
            words("flint --project ../game render examples/a.scene.toml --project=x --ascii");
        assert_eq!(
            subcommand_words(argv),
            words("examples/a.scene.toml --ascii")
        );
    }

    #[test]
    fn a_variant_overrides_the_base_and_adds_to_repeatable_flags() {
        let mut w = words("examples/a.scene.toml --width 640 --style ascii:cell=8");
        w.extend(words(
            "--width 800 --style ascii:ramp=blocks --output x.png",
        ));
        let args = parse_render(w).unwrap();
        assert_eq!(args.width, Some(800));
        assert_eq!(args.style.styles.len(), 2);
        assert_eq!(args.output, "x.png");
        assert_eq!(args.scene, "examples/a.scene.toml");
    }

    #[test]
    fn slugs_are_file_names_and_unique() {
        let v = |l: &str| Variant {
            label: l.into(),
            flags: vec![],
        };
        assert_eq!(
            slugs(&[v("Full screen!"), v("full  screen"), v("??"), v("MSAA 4x")]),
            ["full-screen", "full-screen-2", "variant", "msaa-4x"]
        );
    }

    #[test]
    fn a_variants_file_takes_lines_or_words() {
        let loaded = parse_file(
            r#"
            title = "ASCII"
            zoom = ["0,0,10,10@2"]
            [[variant]]
            label = "authored"
            [[variant]]
            label = "full"
            flags = "--ascii-scope screen"
            [[variant]]
            label = "cells"
            flags = ["--style", "ascii:cell=12"]
            "#,
        )
        .unwrap();
        assert_eq!(loaded.title.as_deref(), Some("ASCII"));
        assert_eq!(loaded.zooms.len(), 1);
        assert_eq!(loaded.variants.len(), 3);
        assert!(loaded.variants[0].flags.is_empty());
        assert_eq!(loaded.variants[1].flags, ["--ascii-scope", "screen"]);
        assert_eq!(loaded.variants[2].flags, ["--style", "ascii:cell=12"]);
        assert!(parse_file("[[variant]]\nlabel = \"x\"\nflag = \"--ascii\"").is_err());
        assert!(parse_file("[[variant]]\nlabel = \"x\"\nflags = \"scene.toml\"").is_err());
    }

    #[test]
    fn the_outline_sits_just_outside_the_region() {
        let mut cell = RgbaImage::new(20, 20);
        let z = parse_zoom("5,5,10,10").unwrap();
        outline(&mut cell, &z, 1.0);
        assert_eq!(cell.get_pixel(4, 4), &LOCATOR);
        assert_eq!(cell.get_pixel(10, 10), &LOCATOR);
        assert_eq!(cell.get_pixel(5, 5), &Rgba([0, 0, 0, 0]));
        assert_eq!(cell.get_pixel(9, 9), &Rgba([0, 0, 0, 0]));
    }
}
