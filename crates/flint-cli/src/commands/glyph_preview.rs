//! `flint gen <set>.glyphs.toml` (#284): a glyph set's atlas, each ramp's
//! measured ink and a lit sphere drawn through it, from the `flint` CLI.
//!
//! `-o sheet.png` writes one labelled sheet; `-o dir/` writes the sheet and
//! its pieces (`<set>_atlas.png`, `<set>_coverage.json`, and a PNG and a text
//! specimen a ramp); `--dry-run` only prints the summary. All CPU: no GPU,
//! no cargo.

use super::gen::GenArgs;
use super::style_sheet::{draw_text, DIM, FRAME, LABEL, PAPER, TITLE};
use anyhow::{bail, Context, Result};
use flint_render::glyph_atlas::specimen::{
    draw_specimen, is_rising, ramp_coverage, specimen_text, SPECIMEN_COLS,
};
use flint_render::{GlyphAtlas, GlyphRamp, GlyphSet, RampMap};
use image::{Rgba, RgbaImage};
use std::path::{Path, PathBuf};

/// Whether `flint gen` was handed a glyph set rather than a procgen spec.
pub fn is_glyph_set(spec: &str) -> bool {
    spec.to_ascii_lowercase()
        .ends_with(flint_render::glyph_atlas::GLYPHS_SUFFIX)
}

pub fn run(args: &GenArgs) -> Result<()> {
    let spec = args.spec.as_deref().expect("checked by the caller");
    refuse_procgen_flags(args)?;
    let path = Path::new(spec);
    let set = GlyphSet::load(path).with_context(|| format!("failed to load {spec}"))?;
    let say = |line: String| println!("{line}");
    for line in summary(&set) {
        say(line);
    }
    if args.dry_run {
        return Ok(());
    }

    let sheet = compose_sheet(&set, spec);
    match Target::of(args.output.as_deref(), set.name()) {
        Target::Sheet(file) => {
            ensure_parent(&file)?;
            save(&sheet, &file)?;
            say(format!("wrote {}", file.display()));
        }
        Target::Folder(dir) => {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
            let name = set.name();
            let mut wrote = Vec::new();
            let file = dir.join(format!("{name}_sheet.png"));
            save(&sheet, &file)?;
            wrote.push(file);
            let atlas = set.atlas().to_image();
            let big = image::imageops::resize(
                &atlas,
                atlas.width() * 4,
                atlas.height() * 4,
                image::imageops::FilterType::Nearest,
            );
            let file = dir.join(format!("{name}_atlas.png"));
            save(&big, &file)?;
            wrote.push(file);
            let file = dir.join(format!("{name}_coverage.json"));
            let text = serde_json::to_string_pretty(&coverage_report(&set))?;
            std::fs::write(&file, text + "\n")
                .with_context(|| format!("failed to write {}", file.display()))?;
            wrote.push(file);
            for ramp in set.ramps() {
                let file = dir.join(format!("{name}_{}.png", ramp.name()));
                save(&draw_specimen(&set, ramp, specimen_scale(&set)), &file)?;
                wrote.push(file);
                let file = dir.join(format!("{name}_{}.txt", ramp.name()));
                std::fs::write(&file, specimen_text(&set, ramp))
                    .with_context(|| format!("failed to write {}", file.display()))?;
                wrote.push(file);
            }
            for file in wrote {
                say(format!("wrote {}", file.display()));
            }
        }
    }
    Ok(())
}

/// The flags that only mean something to a procgen spec, refused by name
/// rather than ignored.
fn refuse_procgen_flags(args: &GenArgs) -> Result<()> {
    let used: Vec<&str> = [
        (args.ai_assist, "--ai-assist"),
        (args.ai_refine, "--ai-refine"),
        (args.seed.is_some(), "--seed"),
        (args.format.is_some(), "--format"),
        (args.batch.is_some(), "--batch"),
        (args.seed_start.is_some(), "--seed-start"),
        (args.register, "--register"),
        (args.strip, "--strip"),
        (args.print_spec, "--print-spec"),
        (args.save_spec.is_some(), "--save-spec"),
        (args.style_guide.is_some(), "--style-guide"),
        (args.validate || args.report.is_some(), "--validate"),
    ]
    .into_iter()
    .filter_map(|(on, flag)| on.then_some(flag))
    .collect();
    if !used.is_empty() {
        bail!(
            "{} {} for .procgen.toml specs; a .glyphs.toml takes -o and --dry-run",
            used.join(", "),
            if used.len() == 1 { "is" } else { "are" }
        );
    }
    Ok(())
}

/// The summary lines: the set, then each ramp with its ink and whether it
/// rises.
pub fn summary(set: &GlyphSet) -> Vec<String> {
    let atlas = set.atlas();
    let [cw, ch] = atlas.cell_size();
    let [aw, ah] = atlas.size();
    let mut lines = vec![format!(
        "{}: {} glyphs in {cw}x{ch} cells, atlas {aw}x{ah}, edges {}",
        set.name(),
        atlas.glyphs().len(),
        set.edges().all().iter().collect::<String>(),
    )];
    let width = set.ramps().map(|r| r.name().len()).max().unwrap_or(0);
    for ramp in set.ramps() {
        let cov = ramp_coverage(set, ramp);
        let glyphs: String = ramp.glyphs().iter().collect();
        let ink: Vec<String> = cov.iter().map(|v| format!("{v:.2}")).collect();
        let default = if ramp.name() == set.default_ramp().name() {
            " (default)"
        } else {
            ""
        };
        lines.push(format!(
            "  {:width$}  \"{glyphs}\"  ink {}  {}{default}",
            ramp.name(),
            ink.join(" "),
            rise_note(ramp),
        ));
    }
    lines
}

/// Every glyph's measured ink and each ramp's glyphs with theirs, as JSON:
/// `{set, cell, size, glyphs: [{glyph, coverage}], ramps: [{name, glyphs,
/// coverage, rising, map, bounds, inversions, calibrated}]}` (#281's fields as
/// the glyph_atlas example writes them). `rising` says whether the ramp's ink never falls
/// from one step to the next (#281).
pub fn coverage_report(set: &GlyphSet) -> serde_json::Value {
    let atlas = set.atlas();
    let round = |v: f32| (v as f64 * 1e4).round() / 1e4;
    let glyphs: Vec<serde_json::Value> = atlas
        .glyphs()
        .iter()
        .map(|c| {
            serde_json::json!({
                "glyph": c.to_string(),
                "coverage": round(atlas.coverage(*c).unwrap_or(0.0)),
            })
        })
        .collect();
    let ramps: Vec<serde_json::Value> = set
        .ramps()
        .map(|r| {
            let cov = ramp_coverage(set, r);
            let calibrated = r.calibrated();
            serde_json::json!({
                "name": r.name(),
                "glyphs": r.glyphs().iter().collect::<String>(),
                "coverage": cov.iter().map(|v| round(*v)).collect::<Vec<_>>(),
                "rising": is_rising(&cov),
                "map": match r.map() {
                    RampMap::Index => "index",
                    RampMap::Coverage => "coverage",
                },
                "bounds": r.bounds().iter().map(|v| round(*v)).collect::<Vec<_>>(),
                "inversions": r
                    .inversions()
                    .iter()
                    .map(|(a, b)| [a.to_string(), b.to_string()])
                    .collect::<Vec<_>>(),
                "calibrated": {
                    "glyphs": calibrated.glyphs().iter().collect::<String>(),
                    "bounds": calibrated.bounds().iter().map(|v| round(*v)).collect::<Vec<_>>(),
                },
            })
        })
        .collect();
    serde_json::json!({
        "set": set.name(),
        "cell": atlas.cell_size(),
        "size": atlas.size(),
        "default_ramp": set.default_ramp().name(),
        "glyphs": glyphs,
        "ramps": ramps,
    })
}

/// "rises" (or "calibrated" for a `map = "coverage"` ramp, #281), or the
/// neighbours where a brighter input gets less ink, from
/// [`GlyphRamp::inversions`].
fn rise_note(ramp: &GlyphRamp) -> String {
    if ramp.map() == RampMap::Coverage {
        return "calibrated (map = coverage)".into();
    }
    let inversions = ramp.inversions();
    if inversions.is_empty() {
        return "rises".into();
    }
    let falls: Vec<String> = inversions.iter().map(|(a, b)| format!("{a}>{b}")).collect();
    format!("falls at {}; map = \"coverage\" fixes it", falls.join(" "))
}

enum Target {
    Sheet(PathBuf),
    Folder(PathBuf),
}

impl Target {
    /// A path ending in a separator, or an existing folder, gets every file;
    /// any other path is the sheet; none is `<set>_sheet.png` here.
    fn of(output: Option<&str>, name: &str) -> Target {
        match output {
            None => Target::Sheet(PathBuf::from(format!("{name}_sheet.png"))),
            Some(o) if o.ends_with('/') || o.ends_with('\\') || Path::new(o).is_dir() => {
                Target::Folder(PathBuf::from(o))
            }
            Some(o) => Target::Sheet(PathBuf::from(o)),
        }
    }
}

fn ensure_parent(file: &Path) -> Result<()> {
    if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}

fn save<P, C>(img: &image::ImageBuffer<P, C>, file: &Path) -> Result<()>
where
    P: image::PixelWithColorType,
    [P::Subpixel]: image::EncodableLayout,
    C: std::ops::Deref<Target = [P::Subpixel]>,
{
    img.save(file)
        .with_context(|| format!("failed to write {}", file.display()))
}

/// The specimen's scale: about 1152 pixels across whatever the cell.
fn specimen_scale(set: &GlyphSet) -> u32 {
    let cw = set.atlas().cell_size()[0].max(1);
    (1152 / (SPECIMEN_COLS * cw)).max(1)
}

const MARGIN: u32 = 32;
const GAP: u32 = 24;
const CHART_W: u32 = 560;
/// Built-in font pixels a label glyph.
const FONT: u32 = 8;
const BAR: Rgba<u8> = Rgba([236, 226, 200, 255]);
const FALL: Rgba<u8> = Rgba([236, 92, 84, 255]);
const EDGE: Rgba<u8> = Rgba([255, 176, 92, 255]);

/// One sheet: the set's name and numbers, its atlas and edge glyphs, then a
/// row a ramp of the specimen and the ramp's ink, step by step.
pub fn compose_sheet(set: &GlyphSet, source: &str) -> RgbaImage {
    let atlas = set.atlas();
    let [cw, ch] = atlas.cell_size();
    let spec_scale = specimen_scale(set);
    let specimens: Vec<_> = set
        .ramps()
        .map(|r| (r, draw_specimen(set, r, spec_scale)))
        .collect();
    let (sw, sh) = specimens
        .first()
        .map(|(_, s)| s.dimensions())
        .unwrap_or((0, 0));

    let atlas_img = atlas.to_image();
    let atlas_scale = (480 / atlas_img.width().max(1)).clamp(1, 6);
    let (aw, ah) = (
        atlas_img.width() * atlas_scale,
        atlas_img.height() * atlas_scale,
    );

    let width = MARGIN * 2 + sw + GAP + CHART_W;
    let title_h = FONT * 3 + 10 + FONT * 2 + 6 + FONT * 2 + GAP;
    let label_h = FONT * 2 + 10;
    let row_h = label_h + sh.max(1) + GAP;
    let height =
        MARGIN + title_h + label_h + ah + GAP * 2 + row_h * specimens.len() as u32 + MARGIN;
    let mut sheet = RgbaImage::from_pixel(width, height, PAPER);
    let font = GlyphAtlas::builtin();

    let mut y = MARGIN;
    draw_text(&mut sheet, &font, MARGIN, y, 3, TITLE, set.name());
    y += FONT * 3 + 10;
    draw_text(
        &mut sheet,
        &font,
        MARGIN,
        y,
        2,
        DIM,
        &source.replace('\\', "/"),
    );
    y += FONT * 2 + 6;
    let subtitle = format!(
        "{} glyphs, {cw}x{ch} cells, atlas {}x{}, default ramp {}",
        atlas.glyphs().len(),
        atlas.size()[0],
        atlas.size()[1],
        set.default_ramp().name(),
    );
    draw_text(&mut sheet, &font, MARGIN, y, 2, DIM, &subtitle);
    y += FONT * 2 + GAP;

    // The atlas as packed, and the edge glyphs large beside it.
    draw_text(&mut sheet, &font, MARGIN, y, 2, LABEL, "atlas");
    let edges_x = MARGIN + aw + GAP * 2;
    draw_text(&mut sheet, &font, edges_x, y, 2, LABEL, "edges");
    y += label_h;
    frame(&mut sheet, MARGIN - 1, y - 1, aw + 2, ah + 2);
    for (py, row) in atlas_img.rows().enumerate() {
        for (px, p) in row.enumerate() {
            let c = coverage_ink(p.0[0], LABEL);
            fill(
                &mut sheet,
                MARGIN + px as u32 * atlas_scale,
                y + py as u32 * atlas_scale,
                atlas_scale,
                atlas_scale,
                c,
            );
        }
    }
    let edge_scale = (96 / cw.max(ch).max(1)).max(1);
    for (i, c) in set.edges().all().iter().enumerate() {
        let x = edges_x + i as u32 * (cw * edge_scale + GAP);
        frame(
            &mut sheet,
            x - 1,
            y - 1,
            cw * edge_scale + 2,
            ch * edge_scale + 2,
        );
        blit_glyph(&mut sheet, atlas, *c, x, y, edge_scale, EDGE);
    }
    y += ah + GAP * 2;

    for (ramp, specimen) in &specimens {
        let cov = ramp_coverage(set, ramp);
        let default = if ramp.name() == set.default_ramp().name() {
            "  (default)"
        } else {
            ""
        };
        let label = format!("ramp {}{default}", ramp.name());
        draw_text(&mut sheet, &font, MARGIN, y, 2, LABEL, &label);
        let note = rise_note(ramp);
        let chart_x = MARGIN + sw + GAP;
        let ink = if ramp.inversions().is_empty() {
            DIM
        } else {
            FALL
        };
        draw_text(
            &mut sheet,
            &font,
            chart_x,
            y,
            2,
            ink,
            &format!("ink by step: {note}"),
        );
        y += label_h;
        image::imageops::overlay(
            &mut sheet,
            &image::DynamicImage::ImageRgb8(specimen.clone()).to_rgba8(),
            MARGIN as i64,
            y as i64,
        );
        frame(&mut sheet, MARGIN - 1, y - 1, sw + 2, sh + 2);
        draw_chart(&mut sheet, atlas, ramp, &cov, chart_x, y, CHART_W, sh);
        y += sh + GAP;
    }
    sheet
}

/// A ramp's ink as bars, one a step with its glyph under it, against the
/// straight line from its first step's ink to its last. A step whose ink
/// falls below the one before it draws red.
#[allow(clippy::too_many_arguments)]
fn draw_chart(
    sheet: &mut RgbaImage,
    atlas: &GlyphAtlas,
    ramp: &GlyphRamp,
    cov: &[f32],
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) {
    frame(sheet, x - 1, y - 1, w + 2, h + 2);
    let n = cov.len().max(1) as u32;
    let [cw, ch] = atlas.cell_size();
    let slot = w / n;
    let glyph_scale = ((slot * 3 / 4) / cw.max(1)).clamp(1, 6);
    let glyph_h = ch * glyph_scale;
    let pad = 12;
    let plot_top = y + pad;
    let plot_h = h.saturating_sub(glyph_h + pad * 3);
    let base = plot_top + plot_h;
    let top = cov.iter().cloned().fold(0.0f32, f32::max).max(1e-3);
    // Quarter lines of the plot.
    for q in 0..=4 {
        let ly = base - plot_h * q / 4;
        for lx in (x + 4..x + w - 4).step_by(4) {
            sheet.put_pixel(lx, ly, FRAME);
        }
    }
    for (i, v) in cov.iter().enumerate() {
        let i = i as u32;
        let bar_h = ((v / top) * plot_h as f32).round() as u32;
        let bw = (slot * 3 / 5).max(1);
        let bx = x + i * slot + (slot - bw) / 2;
        let falls = i > 0 && *v < cov[i as usize - 1];
        fill(
            sheet,
            bx,
            base - bar_h,
            bw,
            bar_h.max(1),
            if falls { FALL } else { BAR },
        );
        let gx = x + i * slot + (slot.saturating_sub(cw * glyph_scale)) / 2;
        let glyph = ramp.glyphs()[i as usize];
        blit_glyph(
            sheet,
            atlas,
            glyph,
            gx,
            base + pad,
            glyph_scale,
            if falls { FALL } else { LABEL },
        );
    }
    // The even line, first step to last, dotted.
    if cov.len() > 1 {
        let (a, b) = (cov[0] / top, cov[cov.len() - 1] / top);
        let x0 = x + slot / 2;
        let x1 = x + (n - 1) * slot + slot / 2;
        for lx in (x0..=x1).step_by(3) {
            let t = (lx - x0) as f32 / (x1 - x0).max(1) as f32;
            let ly = base - ((a + (b - a) * t) * plot_h as f32).round() as u32;
            fill(sheet, lx, ly.saturating_sub(1), 2, 2, EDGE);
        }
    }
}

fn blit_glyph(
    sheet: &mut RgbaImage,
    atlas: &GlyphAtlas,
    ch: char,
    x: u32,
    y: u32,
    scale: u32,
    color: Rgba<u8>,
) {
    let [cw, chh] = atlas.cell_size();
    for py in 0..chh {
        for px in 0..cw {
            let a = atlas.sample(ch, px, py).unwrap_or(0);
            if a == 0 {
                continue;
            }
            fill(
                sheet,
                x + px * scale,
                y + py * scale,
                scale,
                scale,
                coverage_ink(a, color),
            );
        }
    }
}

/// Paper blended toward `color` by coverage.
fn coverage_ink(a: u8, color: Rgba<u8>) -> Rgba<u8> {
    let t = a as f32 / 255.0;
    Rgba(std::array::from_fn(|i| {
        if i == 3 {
            255
        } else {
            (PAPER.0[i] as f32 + (color.0[i] as f32 - PAPER.0[i] as f32) * t).round() as u8
        }
    }))
}

fn fill(sheet: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, c: Rgba<u8>) {
    let (sw, sh) = sheet.dimensions();
    for yy in y..(y + h).min(sh) {
        for xx in x..(x + w).min(sw) {
            sheet.put_pixel(xx, yy, c);
        }
    }
}

fn frame(sheet: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32) {
    let (sw, sh) = sheet.dimensions();
    for i in x..(x + w).min(sw) {
        for j in [y, y + h - 1] {
            if j < sh {
                sheet.put_pixel(i, j, FRAME);
            }
        }
    }
    for j in y..(y + h).min(sh) {
        for i in [x, x + w - 1] {
            if i < sw {
                sheet.put_pixel(i, j, FRAME);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_glyph_sets_take_this_route() {
        assert!(is_glyph_set("fonts/terminal.glyphs.toml"));
        assert!(is_glyph_set("FONTS\\Term.GLYPHS.TOML"));
        assert!(!is_glyph_set("specs/oak_tree.procgen.toml"));
    }

    #[test]
    fn the_builtin_set_makes_a_sheet_and_a_summary() {
        let set = GlyphSet::builtin();
        let sheet = compose_sheet(&set, "builtin");
        assert!(sheet.width() > 1000 && sheet.height() > 600);
        let lines = summary(&set);
        assert_eq!(lines.len(), 1 + set.ramps().count());
        assert!(lines[0].starts_with("builtin: "));
        assert!(lines.iter().any(|l| l.contains("(default)")));
        let report = coverage_report(&set);
        let ramps = report["ramps"].as_array().unwrap();
        assert_eq!(ramps.len(), set.ramps().count());
        for r in ramps {
            let cov = r["coverage"].as_array().unwrap();
            assert_eq!(cov.len(), r["glyphs"].as_str().unwrap().chars().count());
            assert!(r["rising"].is_boolean());
        }
    }

    #[test]
    fn a_path_with_a_separator_is_a_folder() {
        assert!(matches!(Target::of(Some("out/"), "x"), Target::Folder(_)));
        assert!(matches!(
            Target::of(Some("out/x.png"), "x"),
            Target::Sheet(_)
        ));
        assert!(matches!(Target::of(None, "x"), Target::Sheet(p) if p == Path::new("x_sheet.png")));
    }
}
