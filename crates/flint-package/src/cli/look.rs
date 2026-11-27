//! `flint render`'s point for the package that refines a look toward a
//! reference (`--ai-refine`, #357; the ai package's since #1572).
//!
//! The kernel's render owns the scene, the renderer and the measures; the
//! package owns the loop that proposes edits and what proposes them (a
//! paid model, through its cost meter). So the point is two traits: the
//! package registers a [`LookRefine`] as [`LookRefines`], and the render
//! hands it a [`RefineHost`] that draws a candidate block and scores it
//! against the reference, checks an edit the way `--sweep` checks a
//! column, and composes the sheet of rounds. Without the package the render
//! has no `--ai-refine` (its flags are the package's, `super::flags`).
//!
//! What a distance and a frame's measures are ([`DistanceReport`],
//! [`ImageMeasures`]) is the kernel's: `--report` writes them too, and the
//! refine's log carries them as they are.

use anyhow::Result;
use clap::ArgMatches;
use image::{RgbImage, RgbaImage};
use serde::Serialize;

pub use flint_render::look::Look;

/// The package's refine, as the run's points hold it.
pub struct LookRefines(pub Box<dyn LookRefine>);

/// What the render asks of the package that refines a look.
pub trait LookRefine: Send + Sync {
    /// Whether the line asks for a refine. `flags` are `render`'s matches,
    /// the package's own flags among them (`super::flags::parsed`).
    fn asked(&self, flags: Option<&ArgMatches>) -> bool;

    /// Run the refine the line asked for, drawing through `host`: write the
    /// refined look, its log and the sheet of rounds, and say so.
    fn run(&self, flags: Option<&ArgMatches>, host: &mut dyn RefineHost) -> Result<()>;
}

/// One candidate drawn and scored.
pub struct Drawn {
    pub image: RgbaImage,
    /// Its distance to the reference.
    pub distance: DistanceReport,
    /// Its glyph grid's summary, when the ASCII style drew and the
    /// reference has a grid (`--reference-text`).
    pub ink: Option<String>,
}

/// A look's tune sweep, read as `--sweep` reads one: its key
/// (`ascii.brightness`) and values, as written.
pub struct TuneSweep {
    pub key: String,
    pub values: Vec<String>,
}

/// What the render lends a refine.
pub trait RefineHost {
    /// The look the line refines (`--look`).
    fn look(&self) -> &Look;
    /// The reference as the line named it (`--reference`).
    fn reference_path(&self) -> &str;
    /// The reference picture.
    fn reference(&self) -> &RgbImage;
    /// The reference's glyph grid's summary, when it has one.
    fn reference_ink(&self) -> Option<String>;
    /// `-o`: where the sheet of rounds goes.
    fn output(&self) -> &str;
    /// Draw the scene under `post_process` (a look's block laid over the
    /// scene's own) and score it against the reference.
    fn draw(&mut self, post_process: &toml::Table) -> Result<Drawn>;
    /// A frame's measures.
    fn measure(&self, image: &RgbaImage) -> ImageMeasures;
    /// The measures as one line.
    fn summary(&self, measures: &ImageMeasures) -> String;
    /// Check `key=value` the way `--sweep` checks a column (key known,
    /// value in range, a look's key) and put it into `table`, a look's
    /// `[post_process]`.
    fn apply_edit(&self, table: &mut toml::Table, key: &str, value: &str) -> Result<(), String>;
    /// A tune sweep (`ascii.brightness=0.2|0.4`), or `None` when it doesn't
    /// read as one.
    fn tune(&self, sweep: &str) -> Option<TuneSweep>;
    /// The value `table` gives a look key, when it sets one.
    fn current(&self, table: &toml::Table, key: &str) -> Option<toml::Value>;
    /// A sweep value as TOML, as `--sweep` reads it.
    fn value(&self, text: &str) -> toml::Value;
    /// The cells as one labelled sheet, `--sweep`'s.
    fn sheet(
        &self,
        cells: &[RgbaImage],
        title: String,
        subtitle: String,
        labels: Vec<String>,
    ) -> RgbaImage;
}

/// One frame's image measures.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ImageMeasures {
    /// Mean luma, 0..1.
    pub mean_luminance: f64,
    /// Share of near-black pixels.
    pub near_black: f64,
    /// Share of near-white pixels.
    pub near_white: f64,
    /// Share of pixels whose right + down luma step counts as an edge.
    pub edge_density: f64,
    /// Mean sRGB colour.
    pub mean_rgb: [u8; 3],
}

/// How far one glyph grid is from another, each term 0..1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GlyphDistance {
    /// Earth mover's distance between the cells' ink distributions.
    pub coverage: f64,
    /// Mean ink difference over the layout's regions.
    pub layout: f64,
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
    /// The terms as `colour 4.1 + glyph coverage 6.0 + ...`, in points.
    pub fn terms_line(&self) -> String {
        self.terms
            .iter()
            .map(|t| format!("{} {:.2}", t.term, t.points))
            .collect::<Vec<_>>()
            .join(" + ")
    }
}
