//! Numbers for a frame (#357): what a sweep column or a refine round looks
//! like, measured, so an agent can say "column 2 is 71% near-black" instead
//! of guessing from a contact sheet.
//!
//! Every measure is taken from the very pixels that go onto the sheet, so
//! the report and the picture can't disagree: crop a cell out of the sheet
//! and [`measure`] gives the same numbers back (a test holds that).
//!
//! - **mean luminance**: Rec. 709 luma of the display (sRGB) values, 0..1.
//! - **near black / near white**: the share of pixels whose luma is at most
//!   [`NEAR_BLACK`] / at least [`NEAR_WHITE`] (of 255).
//! - **edge density**: the share of pixels where the luma changes by more
//!   than [`EDGE_STEP`] to the right or below neighbour (the sum of both
//!   steps), which is how busy the frame is: line work, glyphs, texture.
//! - **ASCII**, when the style drew: the grid's size, the share of blank
//!   cells, and the share of cells on each step of the ramp, darkest first;
//!   glyphs not on the ramp (edge lines) are counted apart.
//! - **distance**, with a reference: `flint_procgen_ai::measure`, the
//!   metric `flint gen --ai-refine` uses (#183), a score out of 100.

use flint_procgen_ai::measure::{distance_rgb, Distance};
use image::{RgbImage, RgbaImage};
use serde::Serialize;

/// Luma at or under this (0..255) is near black.
pub const NEAR_BLACK: f64 = 24.0;
/// Luma at or over this (0..255) is near white.
pub const NEAR_WHITE: f64 = 232.0;
/// A luma step (0..255) past which a pixel counts as an edge.
pub const EDGE_STEP: f64 = 24.0;

/// One frame's image measures.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ImageMeasures {
    /// Mean luma, 0..1.
    pub mean_luminance: f64,
    /// Share of pixels with luma <= [`NEAR_BLACK`].
    pub near_black: f64,
    /// Share of pixels with luma >= [`NEAR_WHITE`].
    pub near_white: f64,
    /// Share of pixels whose right + down luma step passes [`EDGE_STEP`].
    pub edge_density: f64,
    /// Mean sRGB colour.
    pub mean_rgb: [u8; 3],
}

fn luma(p: [u8; 3]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

/// Measure an image (alpha ignored).
pub fn measure(img: &RgbaImage) -> ImageMeasures {
    let (w, h) = img.dimensions();
    let n = (f64::from(w) * f64::from(h)).max(1.0);
    let lumas: Vec<f64> = img
        .pixels()
        .map(|p| luma([p.0[0], p.0[1], p.0[2]]))
        .collect();
    let at = |x: u32, y: u32| lumas[(y * w + x) as usize];
    let (mut sum, mut black, mut white, mut edges) = (0.0, 0usize, 0usize, 0usize);
    let mut rgb = [0.0f64; 3];
    for y in 0..h {
        for x in 0..w {
            let l = at(x, y);
            sum += l;
            black += usize::from(l <= NEAR_BLACK);
            white += usize::from(l >= NEAR_WHITE);
            let right = if x + 1 < w {
                (at(x + 1, y) - l).abs()
            } else {
                0.0
            };
            let down = if y + 1 < h {
                (at(x, y + 1) - l).abs()
            } else {
                0.0
            };
            edges += usize::from(right + down > EDGE_STEP);
            let p = img.get_pixel(x, y).0;
            for c in 0..3 {
                rgb[c] += f64::from(p[c]);
            }
        }
    }
    ImageMeasures {
        mean_luminance: sum / n / 255.0,
        near_black: black as f64 / n,
        near_white: white as f64 / n,
        edge_density: edges as f64 / n,
        mean_rgb: rgb.map(|v| (v / n).round().clamp(0.0, 255.0) as u8),
    }
}

/// One step of an ASCII ramp and its share of the cells.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RampStep {
    pub glyph: char,
    pub share: f64,
}

/// What the ASCII grid drew.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AsciiMeasures {
    pub cols: u32,
    pub rows: u32,
    /// The ramp the steps are of (`font/ramp`), when it could be found.
    pub ramp: Option<String>,
    /// Share of cells that are blank (a space, or passed through).
    pub blank: f64,
    /// Share of cells on each ramp step, darkest first. Empty when the ramp
    /// couldn't be found; `glyphs` still counts every character.
    pub steps: Vec<RampStep>,
    /// Share of cells whose glyph isn't on the ramp (edge line glyphs).
    pub off_ramp: f64,
    /// Every glyph drawn and its share, most common first.
    pub glyphs: Vec<RampStep>,
}

/// Measure an ASCII grid; `ramp` is the ramp's glyphs, darkest first.
pub fn measure_ascii(
    text: &flint_render::stylize::GlyphText,
    ramp: Option<(String, Vec<char>)>,
) -> AsciiMeasures {
    let n = (text.cells.len() as f64).max(1.0);
    let mut counts: std::collections::BTreeMap<char, usize> = Default::default();
    let mut blank = 0usize;
    for cell in &text.cells {
        let g = cell.glyph.unwrap_or(flint_render::stylize::text::PASSED);
        if g == ' ' {
            blank += 1;
        }
        *counts.entry(g).or_default() += 1;
    }
    let share = |c: usize| c as f64 / n;
    let mut glyphs: Vec<RampStep> = counts
        .iter()
        .map(|(&glyph, &c)| RampStep {
            glyph,
            share: share(c),
        })
        .collect();
    glyphs.sort_by(|a, b| b.share.total_cmp(&a.share).then(a.glyph.cmp(&b.glyph)));
    let (name, steps, off_ramp) = match ramp {
        Some((name, chars)) => {
            let steps: Vec<RampStep> = chars
                .iter()
                .map(|&glyph| RampStep {
                    glyph,
                    share: share(counts.get(&glyph).copied().unwrap_or(0)),
                })
                .collect();
            let on: usize = counts
                .iter()
                .filter(|(g, _)| chars.contains(g) || **g == ' ')
                .map(|(_, c)| *c)
                .sum();
            (Some(name), steps, share(text.cells.len() - on))
        }
        None => (None, Vec::new(), 0.0),
    };
    AsciiMeasures {
        cols: text.cols,
        rows: text.rows,
        ramp: name,
        blank: share(blank),
        steps,
        off_ramp,
        glyphs,
    }
}

/// A distance to the reference, as written in a report.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DistanceReport {
    /// Out of 100; 0 is the reference itself.
    pub score: f64,
    pub mean_colour: f64,
    pub histogram: f64,
    pub layout: f64,
}

impl From<Distance> for DistanceReport {
    fn from(d: Distance) -> Self {
        Self {
            score: d.score,
            mean_colour: d.mean_colour,
            histogram: d.histogram,
            layout: d.layout,
        }
    }
}

/// The distance from `img` to `reference`.
pub fn distance_to(reference: &RgbImage, img: &RgbaImage) -> Distance {
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    distance_rgb(reference, &rgb)
}

/// Load a reference image.
pub fn load_reference(path: &str) -> anyhow::Result<RgbImage> {
    use anyhow::Context;
    Ok(image::open(path)
        .with_context(|| format!("--reference: reading {path}"))?
        .to_rgb8())
}

/// One column of a report: its value and what it measured.
#[derive(Debug, Clone, Serialize)]
pub struct ColumnReport {
    /// The column's index, left to right, top to bottom on the sheet.
    pub index: usize,
    /// The value swept (`None` for a single frame).
    pub value: Option<String>,
    pub image: ImageMeasures,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ascii: Option<AsciiMeasures>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<DistanceReport>,
}

/// `flint render --report FILE` (#357).
#[derive(Debug, Clone, Serialize)]
pub struct RenderReport {
    pub scene: String,
    pub look: Option<String>,
    /// The swept key, when there was a sweep.
    pub sweep: Option<String>,
    pub reference: Option<String>,
    /// Each column's size in pixels.
    pub width: u32,
    pub height: u32,
    /// The thresholds the shares were counted at (of 255).
    pub thresholds: Thresholds,
    pub columns: Vec<ColumnReport>,
    /// The column nearest the reference, when there is one.
    pub nearest: Option<usize>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Thresholds {
    pub near_black: f64,
    pub near_white: f64,
    pub edge_step: f64,
}

impl Thresholds {
    pub const USED: Self = Self {
        near_black: NEAR_BLACK,
        near_white: NEAR_WHITE,
        edge_step: EDGE_STEP,
    };
}

impl RenderReport {
    /// The index of the column with the lowest distance.
    pub fn nearest_column(columns: &[ColumnReport]) -> Option<usize> {
        columns
            .iter()
            .filter_map(|c| c.distance.map(|d| (c.index, d.score)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }
}

/// A short line for the terminal and a sheet label.
pub fn summary(m: &ImageMeasures, ascii: Option<&AsciiMeasures>) -> String {
    let mut s = format!(
        "luma {:.2}  black {:.0}%  white {:.0}%  edges {:.0}%",
        m.mean_luminance,
        m.near_black * 100.0,
        m.near_white * 100.0,
        m.edge_density * 100.0
    );
    if let Some(a) = ascii {
        s.push_str(&format!("  blank cells {:.0}%", a.blank * 100.0));
    }
    s
}

/// The ASCII style's ramp for `config`, darkest first: its `font` and
/// `ramp` options (the set's default ramp when none is named), looked up
/// beside the scene as the style does.
pub fn ascii_ramp(
    config: &flint_render::PostProcessConfig,
    scene: &std::path::Path,
) -> Option<(String, Vec<char>)> {
    let font = config
        .styles
        .option("ascii", "font")
        .filter(|f| !f.is_empty())
        .unwrap_or("builtin")
        .to_string();
    let ramp = config
        .styles
        .option("ascii", "ramp")
        .unwrap_or("")
        .to_string();
    let mut cache = flint_render::glyph_atlas::GlyphSetCache::for_scene(scene);
    let set = cache.get(&font).ok()?;
    let r = set.ramp(&ramp).unwrap_or_else(|| set.default_ramp());
    Some((format!("{font}/{}", r.name()), r.glyphs().to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn a_half_black_half_white_frame() {
        let img = RgbaImage::from_fn(8, 4, |x, _| {
            if x < 4 {
                Rgba([0, 0, 0, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        });
        let m = measure(&img);
        assert_eq!(m.near_black, 0.5);
        assert_eq!(m.near_white, 0.5);
        assert!((m.mean_luminance - 0.5).abs() < 1e-9);
        // One column of pixels (x = 3) steps to white on its right
        assert_eq!(m.edge_density, 4.0 / 32.0);
    }

    #[test]
    fn a_cell_cropped_from_the_sheet_measures_the_same() {
        // Five cells (two rows), each a different noisy picture
        let cells: Vec<RgbaImage> = (0..5u32)
            .map(|k| {
                RgbaImage::from_fn(96, 54, |x, y| {
                    let v = ((x * 7 + y * 13 + k * 31) * (k + 3) % 256) as u8;
                    Rgba([v, v / 2, 255 - v, 255])
                })
            })
            .collect();
        let mut text = crate::commands::style_sheet::SheetText::new(
            "t",
            "s",
            (0..5).map(|i| format!("v{i}  d 1.0 *")).collect(),
        );
        // Notes add a band under each row (#216); the crop still finds the cells
        text.notes = vec!["note".into(); 5];
        let sheet = crate::commands::style_sheet::compose(&cells, &text);
        for (i, cell) in cells.iter().enumerate() {
            let (x, y) = crate::commands::style_sheet::cell_origin(96, 54, &text, i);
            let crop = image::imageops::crop_imm(&sheet, x, y, 96, 54).to_image();
            assert_eq!(measure(&crop), measure(cell), "cell {i}");
        }
    }

    #[test]
    fn a_flat_grey_has_no_edges_and_no_extremes() {
        let m = measure(&RgbaImage::from_pixel(5, 5, Rgba([128, 128, 128, 255])));
        assert_eq!(
            (m.near_black, m.near_white, m.edge_density),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(m.mean_rgb, [128, 128, 128]);
    }

    #[test]
    fn ascii_steps_count_cells_on_the_ramp() {
        use flint_render::stylize::text::{GlyphCell, GlyphText};
        let cell = |g: char| GlyphCell {
            glyph: Some(g),
            ink: [255; 3],
            ground: [0; 3],
        };
        let text = GlyphText {
            cols: 4,
            rows: 1,
            cells: vec![cell(' '), cell(' '), cell('#'), cell('/')],
        };
        let a = measure_ascii(&text, Some(("t".into(), vec![' ', '.', '#'])));
        assert_eq!(a.blank, 0.5);
        assert_eq!(
            a.steps.iter().map(|s| s.share).collect::<Vec<_>>(),
            [0.5, 0.0, 0.25]
        );
        assert_eq!(a.off_ramp, 0.25, "the edge glyph");
        assert_eq!(a.glyphs[0].glyph, ' ');
    }
}
