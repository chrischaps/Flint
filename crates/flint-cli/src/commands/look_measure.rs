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
//!   metric `flint gen --ai-refine` uses (#183), a score out of 100. With
//!   the reference's glyph grid too (`--reference-text`, #495) and a frame
//!   that drew one, the score reads glyphs rather than pixels: mean colour,
//!   glyph coverage and glyph layout ([`glyph_distance`]). Either way the
//!   report lists each term and its points.

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

// --- Glyphs as ink (#495) ---------------------------------------------------
//
// #183's distance reads an ASCII frame as pixels: a glyph is texture, and a
// halo that smears ink into the ground moves its histogram more than a
// change of letters does. When both frames are glyph grids, the score reads
// the grids instead: each cell is the ink of the glyph it shows (its
// coverage in the atlas), and two terms compare those inks.
//
// - **glyph coverage**: the earth mover's distance between the two grids'
//   distributions of cell ink, so how much ink would have to move, and how
//   far, for one grid's mix of blank, light and dense letters to become the
//   other's. A near miss (`+` for `*`) costs less than a far one.
// - **glyph layout**: the mean difference of the two grids' ink over 16x16
//   regions of the frame, which is where the dense letters sit (a sky in
//   `@`, a floor of dots).
//
// The grids' cells needn't match: a share and a region are fractions of the
// frame, so a cell of 9 compares with a cell of 8.

/// Side of the region grid the glyph layout term compares.
pub const GLYPH_LAYOUT: u32 = 16;

/// What a glyph's ink is read as: its coverage in a glyph set's atlas,
/// over the densest printable ASCII letter's, so a letter grid spans 0
/// (blank) to 1 (`@`, `M`); a solid block reads past it and is held at 1.
pub struct InkScale {
    set: std::sync::Arc<flint_render::glyph_atlas::GlyphSet>,
    builtin: std::sync::Arc<flint_render::glyph_atlas::GlyphSet>,
    full: f32,
}

/// What a glyph no atlas knows counts as: halfway.
pub const UNKNOWN_INK: f32 = 0.5;

impl InkScale {
    /// The built-in glyph set's.
    pub fn builtin() -> Self {
        Self::of(flint_render::glyph_atlas::GlyphSet::builtin())
    }

    /// The scale of `set`, with the built-in set for a glyph it lacks.
    pub fn of(set: std::sync::Arc<flint_render::glyph_atlas::GlyphSet>) -> Self {
        let full = (0x21u8..=0x7e)
            .filter_map(|c| set.atlas().coverage(char::from(c)))
            .fold(0.0f32, f32::max);
        let builtin = flint_render::glyph_atlas::GlyphSet::builtin();
        let full = if full > 0.0 {
            full
        } else {
            (0x21u8..=0x7e)
                .filter_map(|c| builtin.atlas().coverage(char::from(c)))
                .fold(0.0f32, f32::max)
                .max(1e-3)
        };
        Self { set, builtin, full }
    }

    /// The glyph set the ASCII style draws with under `config` (its `font`
    /// option, found beside the scene as the style finds it); the built-in
    /// set when it can't be found.
    pub fn for_config(config: &flint_render::PostProcessConfig, scene: &std::path::Path) -> Self {
        let font = config
            .styles
            .option("ascii", "font")
            .filter(|f| !f.is_empty())
            .unwrap_or("builtin");
        let mut cache = flint_render::glyph_atlas::GlyphSetCache::for_scene(scene);
        match cache.get(font) {
            Ok(set) => Self::of(set),
            Err(_) => Self::builtin(),
        }
    }

    /// The ink of `ch`, 0..1.
    pub fn ink(&self, ch: char) -> f32 {
        if ch == ' ' || ch == flint_render::stylize::text::PASSED {
            return 0.0;
        }
        let coverage = self
            .set
            .atlas()
            .coverage(ch)
            .or_else(|| self.builtin.atlas().coverage(ch));
        match coverage {
            Some(c) => (c / self.full).clamp(0.0, 1.0),
            None => UNKNOWN_INK,
        }
    }
}

/// A glyph grid as ink: each cell's glyph read through an [`InkScale`].
#[derive(Debug, Clone, PartialEq)]
pub struct InkGrid {
    pub cols: u32,
    pub rows: u32,
    /// Each cell's ink, 0..1, row by row from the top left.
    pub ink: Vec<f32>,
    /// How many cells show each glyph, most first (blank as ' ').
    pub glyphs: Vec<(char, usize)>,
}

impl InkGrid {
    /// What a frame's grid drew; a passed-through cell is blank.
    pub fn from_text(text: &flint_render::stylize::GlyphText, scale: &InkScale) -> Self {
        let chars = text
            .cells
            .iter()
            .map(|c| c.glyph.unwrap_or(flint_render::stylize::text::PASSED));
        Self::from_chars(text.cols, text.rows, chars, scale)
    }

    /// A grid written by `--ascii-text` (#317): a line per cell row. A short
    /// line is blank past its end. `None` when it has no cells.
    pub fn parse(text: &str, scale: &InkScale) -> Option<Self> {
        let lines: Vec<Vec<char>> = text
            .lines()
            .map(|l| l.trim_end_matches('\r').chars().collect())
            .collect();
        let cols = lines.iter().map(Vec::len).max().unwrap_or(0) as u32;
        let rows = lines.len() as u32;
        if cols == 0 || rows == 0 {
            return None;
        }
        let chars = lines
            .iter()
            .flat_map(|l| (0..cols as usize).map(move |x| l.get(x).copied().unwrap_or(' ')));
        Some(Self::from_chars(cols, rows, chars, scale))
    }

    /// A grid with nothing drawn: what a frame that draws no glyphs is
    /// against a reference that does.
    pub fn blank(cols: u32, rows: u32) -> Self {
        let n = (cols * rows) as usize;
        Self {
            cols,
            rows,
            ink: vec![0.0; n],
            glyphs: vec![(' ', n)],
        }
    }

    fn from_chars(
        cols: u32,
        rows: u32,
        chars: impl Iterator<Item = char>,
        scale: &InkScale,
    ) -> Self {
        let mut counts: std::collections::BTreeMap<char, usize> = Default::default();
        let ink = chars
            .map(|c| {
                *counts.entry(c).or_default() += 1;
                scale.ink(c)
            })
            .collect();
        let mut glyphs: Vec<(char, usize)> = counts.into_iter().collect();
        glyphs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Self {
            cols,
            rows,
            ink,
            glyphs,
        }
    }

    /// Share of cells that are blank.
    pub fn blank_share(&self) -> f64 {
        let n = self.ink.len().max(1) as f64;
        self.ink.iter().filter(|&&i| i <= 0.0).count() as f64 / n
    }

    /// Mean ink over the frame.
    pub fn mean_ink(&self) -> f64 {
        let n = self.ink.len().max(1) as f64;
        self.ink.iter().map(|&i| f64::from(i)).sum::<f64>() / n
    }

    /// Mean ink in each of [`GLYPH_LAYOUT`]² regions of the frame, each cell
    /// shared among the regions it overlaps by area, so a grid of any size
    /// fills every region and two grids always compare region for region.
    fn regions(&self) -> Vec<f64> {
        let n = GLYPH_LAYOUT as usize;
        if self.cols == 0 || self.rows == 0 {
            return vec![0.0; n * n];
        }
        // The regions a span of cells covers, each with the overlap's length
        // (in region units)
        let spans = |cells: u32| -> Vec<Vec<(usize, f64)>> {
            let per = n as f64 / f64::from(cells);
            (0..cells)
                .map(|i| {
                    let (a, b) = (f64::from(i) * per, f64::from(i + 1) * per);
                    let first = (a.floor() as usize).min(n - 1);
                    let last = ((b.ceil() as usize).max(first + 1)).min(n);
                    (first..last)
                        .map(|r| (r, (b.min(r as f64 + 1.0) - a.max(r as f64)).max(0.0)))
                        .filter(|&(_, w)| w > 0.0)
                        .collect()
                })
                .collect()
        };
        let (xs, ys) = (spans(self.cols), spans(self.rows));
        let mut sum = vec![0.0f64; n * n];
        let mut area = vec![0.0f64; n * n];
        for (y, ry) in ys.iter().enumerate() {
            for (x, rx) in xs.iter().enumerate() {
                let ink = f64::from(self.ink[y * self.cols as usize + x]);
                for &(r, wy) in ry {
                    for &(c, wx) in rx {
                        sum[r * n + c] += ink * wx * wy;
                        area[r * n + c] += wx * wy;
                    }
                }
            }
        }
        sum.iter()
            .zip(&area)
            .map(|(&s, &a)| if a > 0.0 { s / a } else { 0.0 })
            .collect()
    }

    /// One line for an agent or the terminal: blank share, mean ink and
    /// the commonest glyphs.
    pub fn summary(&self) -> String {
        let n = self.ink.len().max(1) as f64;
        let top: Vec<String> = self
            .glyphs
            .iter()
            .filter(|(c, _)| *c != ' ')
            .take(8)
            .map(|(c, k)| format!("'{c}' {:.0}%", *k as f64 / n * 100.0))
            .collect();
        format!(
            "{}x{} cells, blank {:.0}%, mean ink {:.2}, commonest {}",
            self.cols,
            self.rows,
            self.blank_share() * 100.0,
            self.mean_ink(),
            top.join(", ")
        )
    }
}

/// How far one glyph grid is from another, each term 0..1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GlyphDistance {
    /// Earth mover's distance between the cells' ink distributions.
    pub coverage: f64,
    /// Mean ink difference over [`GLYPH_LAYOUT`]² regions.
    pub layout: f64,
}

/// The distance from `current`'s glyphs to `reference`'s.
pub fn glyph_distance(reference: &InkGrid, current: &InkGrid) -> GlyphDistance {
    // In one dimension the earth mover's distance is the area between the
    // two cumulative distributions; sorted samples give it exactly
    let sorted = |g: &InkGrid| {
        let mut v: Vec<f64> = g.ink.iter().map(|&i| f64::from(i)).collect();
        v.sort_by(f64::total_cmp);
        v
    };
    let (a, b) = (sorted(reference), sorted(current));
    let coverage = if a.is_empty() || b.is_empty() {
        0.0
    } else {
        // Step through the union of breakpoints of both CDFs
        let (mut i, mut j, mut last, mut area) = (0usize, 0usize, 0.0f64, 0.0f64);
        let (na, nb) = (a.len() as f64, b.len() as f64);
        while i < a.len() || j < b.len() {
            let next = match (a.get(i), b.get(j)) {
                (Some(&x), Some(&y)) => x.min(y),
                (Some(&x), None) => x,
                (None, Some(&y)) => y,
                (None, None) => unreachable!(),
            };
            area += (i as f64 / na - j as f64 / nb).abs() * (next - last);
            last = next;
            while a.get(i).is_some_and(|&x| x <= next) {
                i += 1;
            }
            while b.get(j).is_some_and(|&y| y <= next) {
                j += 1;
            }
        }
        area
    };
    let (ra, rb) = (reference.regions(), current.regions());
    let layout = ra.iter().zip(&rb).map(|(x, y)| (x - y).abs()).sum::<f64>() / ra.len() as f64;
    GlyphDistance { coverage, layout }
}

/// Which terms a score is the mean of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Metric {
    /// #183's: mean colour, luminance histogram, luminance layout.
    Pixels,
    /// Both frames are glyph grids (#495): mean colour, glyph coverage,
    /// glyph layout. The pixel histogram and layout are still reported.
    Glyphs,
}

impl Metric {
    pub fn name(self) -> &'static str {
        match self {
            Metric::Pixels => "pixels",
            Metric::Glyphs => "glyphs",
        }
    }
}

/// One term's part of a score.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Term {
    pub term: &'static str,
    /// The term itself, 0..1.
    pub distance: f64,
    /// What it adds to the score, in points of 100; a score's terms sum to it.
    pub points: f64,
}

/// A distance to the reference, as written in a report: the score, the
/// terms it is made of, and the pixel terms whichever metric was used.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DistanceReport {
    /// Out of 100; 0 is the reference itself.
    pub score: f64,
    pub metric: Metric,
    /// The terms the score is the mean of, each with its points.
    pub terms: Vec<Term>,
    /// sRGB units, 0..441.7.
    pub mean_colour: f64,
    pub histogram: f64,
    pub layout: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glyph: Option<GlyphDistance>,
}

impl DistanceReport {
    /// The pixel distance alone, or with both grids, the glyph metric.
    pub fn new(d: Distance, glyph: Option<GlyphDistance>) -> Self {
        let colour = d.mean_colour / MAX_RGB_DISTANCE;
        let (metric, parts) = match glyph {
            None => (
                Metric::Pixels,
                vec![
                    ("mean colour", colour),
                    ("histogram", d.histogram),
                    ("layout", d.layout),
                ],
            ),
            Some(g) => (
                Metric::Glyphs,
                vec![
                    ("mean colour", colour),
                    ("glyph coverage", g.coverage),
                    ("glyph layout", g.layout),
                ],
            ),
        };
        let n = parts.len() as f64;
        let terms: Vec<Term> = parts
            .into_iter()
            .map(|(term, distance)| Term {
                term,
                distance,
                points: distance / n * 100.0,
            })
            .collect();
        Self {
            score: terms.iter().map(|t| t.points).sum(),
            metric,
            terms,
            mean_colour: d.mean_colour,
            histogram: d.histogram,
            layout: d.layout,
            glyph,
        }
    }

    /// The terms as `colour 4.1 + glyph coverage 6.0 + ...`, in points.
    pub fn terms_line(&self) -> String {
        self.terms
            .iter()
            .map(|t| format!("{} {:.2}", t.term, t.points))
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

impl From<Distance> for DistanceReport {
    fn from(d: Distance) -> Self {
        Self::new(d, None)
    }
}

/// The largest mean-colour distance (black to white, sRGB 0..255).
const MAX_RGB_DISTANCE: f64 = 441.672_955_930_063_7;

/// The distance from `img` to `reference` (#183's, by pixels).
pub fn distance_to(reference: &RgbImage, img: &RgbaImage) -> Distance {
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    distance_rgb(reference, &rgb)
}

/// What a frame is measured against: the reference picture, and its glyph
/// grid when there is one (`--reference-text`).
pub struct Reference {
    pub image: RgbImage,
    pub ink: Option<InkGrid>,
}

impl Reference {
    /// The distance from a frame (its picture, and its grid when the ASCII
    /// style drew) to the reference. With a reference grid the score is the
    /// glyph metric, and a frame with no grid is a blank one.
    pub fn distance(&self, img: &RgbaImage, ink: Option<&InkGrid>) -> DistanceReport {
        let d = distance_to(&self.image, img);
        let glyph = self.ink.as_ref().map(|r| match ink {
            Some(c) => glyph_distance(r, c),
            None => glyph_distance(r, &InkGrid::blank(r.cols, r.rows)),
        });
        DistanceReport::new(d, glyph)
    }
}

/// Read `--reference-text` (an `--ascii-text` file) into a grid.
pub fn load_reference_text(path: &str, scale: &InkScale) -> anyhow::Result<InkGrid> {
    use anyhow::Context;
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("--reference-text: reading {path}"))?;
    InkGrid::parse(&text, scale).with_context(|| format!("--reference-text: {path} has no cells"))
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
            .filter_map(|c| c.distance.as_ref().map(|d| (c.index, d.score)))
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

    fn grid(text: &str) -> InkGrid {
        InkGrid::parse(text, &InkScale::builtin()).unwrap()
    }

    #[test]
    fn a_glyph_reads_as_its_ink() {
        let s = InkScale::builtin();
        assert_eq!(s.ink(' '), 0.0);
        let (dot, hash, at) = (s.ink('.'), s.ink('#'), s.ink('@'));
        assert!(0.0 < dot && dot < hash && hash <= 1.0, "{dot} {hash}");
        assert!(at > 0.5, "a dense letter is near full: {at}");
        // Printable ASCII tops out at 1; a block may pass it and is held there
        let densest = (0x21u8..=0x7e)
            .map(|c| s.ink(char::from(c)))
            .fold(0.0f32, f32::max);
        assert_eq!(densest, 1.0);
        assert_eq!(s.ink('\u{e000}'), UNKNOWN_INK, "a glyph no atlas has");
    }

    #[test]
    fn an_ascii_text_file_parses_short_lines_as_blank() {
        let g = grid("##\r\n#\n");
        assert_eq!((g.cols, g.rows), (2, 2));
        assert_eq!(g.ink[3], 0.0);
        assert_eq!(g.blank_share(), 0.25);
        assert_eq!(g.glyphs[0], ('#', 3));
        assert!(InkGrid::parse("", &InkScale::builtin()).is_none());
    }

    #[test]
    fn a_grid_is_zero_from_itself_and_far_from_blank() {
        let dense = grid("@@@@\n@@@@\n");
        let d = glyph_distance(&dense, &dense);
        assert_eq!((d.coverage, d.layout), (0.0, 0.0));
        let ink = f64::from(InkScale::builtin().ink('@'));
        let d = glyph_distance(&dense, &InkGrid::blank(4, 2));
        assert!((d.coverage - ink).abs() < 1e-6, "{d:?}");
        assert!((d.layout - ink).abs() < 1e-6, "{d:?}");
    }

    #[test]
    fn a_near_letter_is_closer_than_a_far_one() {
        // Earth mover's: '#' for '@' is a short move, '.' for '@' a long one
        let r = grid("@@@@\n");
        let near = glyph_distance(&r, &grid("####\n")).coverage;
        let far = glyph_distance(&r, &grid("....\n")).coverage;
        assert!(near < far, "{near} vs {far}");
    }

    #[test]
    fn grids_of_different_cells_compare_by_the_frame() {
        // The same picture at two cell sizes: dense on the left, blank on the right
        let coarse = grid("@@  \n@@  \n");
        let fine = grid("@@@@    \n@@@@    \n@@@@    \n@@@@    \n");
        let d = glyph_distance(&coarse, &fine);
        assert!(d.coverage < 1e-9 && d.layout < 1e-9, "{d:?}");
        // Mirrored, the mix is the same and the layout isn't
        let mirrored = grid("  @@\n  @@\n");
        let d = glyph_distance(&coarse, &mirrored);
        assert!(d.coverage < 1e-9 && d.layout > 0.5, "{d:?}");
    }

    #[test]
    fn the_score_is_its_terms_and_pixels_keep_183s_score() {
        let a = RgbImage::from_pixel(8, 8, image::Rgb([200, 40, 40]));
        let b = RgbaImage::from_pixel(8, 8, Rgba([20, 40, 90, 255]));
        let pixels = distance_to(&a, &b);
        let report = DistanceReport::from(pixels);
        assert_eq!(report.metric, Metric::Pixels);
        assert!((report.score - pixels.score).abs() < 1e-9);
        let sum: f64 = report.terms.iter().map(|t| t.points).sum();
        assert!((sum - report.score).abs() < 1e-9);

        // A reference grid scores by glyphs; a frame with none is a blank one
        let reference = Reference {
            image: a,
            ink: Some(grid("@@\n@@\n")),
        };
        let d = reference.distance(&b, None);
        assert_eq!(d.metric, Metric::Glyphs);
        assert_eq!(
            d.terms.iter().map(|t| t.term).collect::<Vec<_>>(),
            ["mean colour", "glyph coverage", "glyph layout"]
        );
        let same = reference.distance(&b, Some(&grid("@@\n@@\n")));
        assert!(same.score < d.score);
        assert!(
            (same.score - same.terms[0].points).abs() < 1e-9,
            "only colour left"
        );
        // The pixel terms are still there to read
        assert!((same.histogram - pixels.histogram).abs() < 1e-9);
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
