//! `flint render --sweep` (#279, #216, unified by #353): one setting at several
//! values, a column each, side by side on one contact sheet.
//!
//! Every family of setting shares the one flag, told apart by the key's prefix:
//!
//! ```text
//! --sweep look=ascii_noir,matrix          a look over the scene, a column each
//! --sweep post.exposure=0.8,1.2           a [post_process] field (`|` splits vectors)
//! --sweep ascii.cell=6,8,12               any registered style's param or option
//! --sweep wooze.glide=0,0.25,0.5,1        a Living Props wooze knob (#216)
//! --sweep living.echoes.spread=0,0.9,1.8  the scene's `[living] echoes` (#531)
//! ```
//!
//! What a key means is decided here, before a scene loads or a GPU wakes, so a
//! typo costs nothing. How a column takes its value is up to its family: a look,
//! post or style column changes the `[post_process]` it draws with
//! ([`Sweep::column_def`], [`Sweep::column_style`]); a wooze column lays
//! [`Sweep::wooze_table`] over the scene's `[wooze]`, and so draws through
//! `render.rs`'s stages, a world and renderer a column (#487); an echoes column
//! is written into its own copy of the scene's `[living]` table
//! ([`SweepGrid::lay_living`]) before its stage loads, so it draws what a scene
//! with that table draws. The sheet the
//! columns land on is [`super::style_sheet::compose`] for every family.

use super::look::{did_you_mean, find, overlay, Look};
use anyhow::{Context, Result};
use flint_scene::PostProcessDef;

/// What a sweep varies.
#[derive(Debug, Clone, PartialEq)]
pub enum SweepKey {
    /// `look=a,b,c`: each column a different look over the scene.
    Look,
    /// `<style>.<key>=...`: a registered style's param or option.
    Style { style: String, key: String },
    /// `post.<field>=...`: a `[post_process]` field.
    Post { field: String },
    /// `wooze.<knob>=...`: one of the scene's global wooze values (#216), as
    /// `WoozeField::set_global` names it.
    Wooze { knob: String },
    /// `living.echoes.<knob>=...`: one field of the scene's `[living] echoes`
    /// table (#531), as a scene would write it.
    Echoes { knob: String },
}

/// `--sweep KEY=V1,V2,...`, parsed and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Sweep {
    pub key: SweepKey,
    /// One per column, as written on the command line.
    pub values: Vec<String>,
}

/// Most columns a sheet takes; past this the cells get too small to judge.
pub const MAX_COLUMNS: usize = 12;

/// Each column's size unless `--width` / `--height` say: small enough that a
/// sheet of them fits a screen, wide enough to judge a style's cells.
pub const CELL_SIZE: (u32, u32) = (640, 360);

/// The knobs `wooze.<knob>` can turn.
pub const WOOZE_KNOBS: [&str; 5] = ["amplitude", "step", "glide", "jitter", "preset"];

/// The fields `living.echoes.<knob>` can turn: `[living] echoes`' own.
pub const ECHO_KNOBS: [&str; 3] = ["count", "spread", "alpha"];

/// `clap`'s value parser for `--sweep`.
pub fn parse_sweep(spec: &str) -> Result<Sweep, String> {
    Sweep::parse(spec)
}

impl Sweep {
    /// Parse `key=v1,v2,...`. Values split on `|` instead when one appears,
    /// so a vector can be swept: `post.fog_color=0,0,0|0.7,0.75,0.82`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let (key, values) = spec.split_once('=').ok_or_else(|| {
            format!(
                "expected KEY=V1,V2,... (got '{spec}'): e.g. ascii.cell=6,8,12, \
                 wooze.glide=0,0.25,0.5,1 or living.echoes.spread=0,0.9,1.8"
            )
        })?;
        let key = key.trim();
        let separator = if values.contains('|') { '|' } else { ',' };
        let values: Vec<String> = values
            .split(separator)
            .map(|v| v.trim().to_string())
            .collect();
        if let Some(blank) = values.iter().position(String::is_empty) {
            return Err(format!(
                "--sweep {key}: value {} of '{spec}' is blank",
                blank + 1
            ));
        }
        if values.len() < 2 {
            return Err(format!("--sweep {key}: give at least two values"));
        }
        if values.len() > MAX_COLUMNS {
            return Err(format!(
                "--sweep {key}: {} values; a sheet takes at most {MAX_COLUMNS}",
                values.len()
            ));
        }
        let key = if key == "look" {
            SweepKey::Look
        } else if let Some(field) = key.strip_prefix("post.") {
            parse_post(field, &values)?
        } else if let Some(knob) = key.strip_prefix("wooze.") {
            parse_wooze(knob, &values)?
        } else if let Some(field) = key.strip_prefix("living.") {
            parse_living(field, &values)?
        } else if let Some((style, k)) = key.split_once('.') {
            parse_style(key, style, k, &values)?
        } else {
            return Err(format!(
                "--sweep: '{key}' is not look, post.<field>, <style>.<key>, wooze.<knob> or \
                 living.echoes.<knob> (e.g. ascii.cell; `--style help` lists the keys)"
            ));
        };
        Ok(Self { key, values })
    }

    /// The key as written.
    pub fn name(&self) -> String {
        match &self.key {
            SweepKey::Look => "look".into(),
            SweepKey::Style { style, key } => format!("{style}.{key}"),
            SweepKey::Post { field } => format!("post.{field}"),
            SweepKey::Wooze { knob } => format!("wooze.{knob}"),
            SweepKey::Echoes { knob } => format!("living.echoes.{knob}"),
        }
    }

    /// What column `i` is labelled with on the sheet: its value, or for a wooze
    /// number the knob and its value (`glide 0.25`), as #216's sheets were,
    /// and an echoes field the same way (`spread 0.9`).
    pub fn label(&self, i: usize) -> String {
        let value = &self.values[i];
        match &self.key {
            SweepKey::Wooze { knob } if knob != "preset" => format!("{knob} {value}"),
            SweepKey::Echoes { knob } => format!("{knob} {value}"),
            _ => value.clone(),
        }
    }

    /// Column `i`'s `[post_process]`: the scene's, under the base look (or
    /// the column's own, for a look sweep), under a post field's value. A
    /// style or wooze column draws with the base's. `flint render` asks
    /// [`SweepGrid::cell_def`], for each axis of a grid.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn column_def(
        &self,
        i: usize,
        scene: Option<&PostProcessDef>,
        base: Option<&Look>,
        scene_path: &str,
    ) -> Result<Option<PostProcessDef>> {
        cell_def(&[(self, i)], scene, base, scene_path)
    }

    /// Column `i`'s `--style` word, for a style sweep.
    pub fn column_style(&self, i: usize) -> Option<String> {
        match &self.key {
            SweepKey::Style { style, key } => Some(format!("{style}:{key}={}", self.values[i])),
            _ => None,
        }
    }

    /// The wooze knob a wooze sweep turns.
    pub fn wooze_knob(&self) -> Option<&str> {
        match &self.key {
            SweepKey::Wooze { knob } => Some(knob),
            _ => None,
        }
    }

    /// What column `i` of a wooze sweep lays over the scene's `[wooze]` table
    /// (#216): the knob at the column's value, a number unless it's a preset's name.
    /// `None` for a sweep of anything else.
    pub fn wooze_table(&self, i: usize) -> Option<toml::Table> {
        let knob = self.wooze_knob()?;
        let text = &self.values[i];
        let value = match text.parse::<f64>() {
            Ok(number) => toml::Value::Float(number),
            Err(_) => toml::Value::String(text.clone()),
        };
        let mut table = toml::Table::new();
        table.insert(knob.to_string(), value);
        Some(table)
    }

    /// The `[living] echoes` field column `i` of an echoes sweep sets, and its
    /// value as a scene would write it: `count` an integer, the others floats.
    /// `None` for a sweep of anything else.
    pub fn echoes_field(&self, i: usize) -> Option<(&str, toml::Value)> {
        let SweepKey::Echoes { knob } = &self.key else {
            return None;
        };
        let text = &self.values[i];
        let value = match knob.as_str() {
            "count" => toml::Value::Integer(text.parse().ok()?),
            _ => toml::Value::Float(text.parse().ok()?),
        };
        Some((knob, value))
    }

    /// Said when columns `i` and `i + 1` came out the same, pixel for pixel: the
    /// sweep showed nothing, and a reader would otherwise think it showed "no
    /// difference". What usually explains it depends on what was swept.
    pub fn identical_warning(&self, i: usize) -> String {
        let why = match &self.key {
            SweepKey::Wooze { .. } => {
                "nothing on screen is reading this knob at this moment, or a zone is keeping \
                 it off these values"
            }
            SweepKey::Echoes { knob } if knob == "count" => {
                "every copy the added echoes would draw has faded to nothing at this spread \
                 and alpha"
            }
            SweepKey::Echoes { .. } => {
                "no living prop draws echoes here: a count of 0, in the scene's [living] \
                 echoes or every spec's, leaves nothing for spread or alpha to change"
            }
            SweepKey::Style { .. } => {
                "the style is off, nothing it draws reads this key, or the style clamps both \
                 values to one"
            }
            SweepKey::Post { .. } => {
                "the effect this field tunes is off, or nothing on screen shows it"
            }
            SweepKey::Look => "the two looks change nothing this scene shows differently",
        };
        format!(
            "{}={} and {} are pixel-identical: {why}",
            self.name(),
            self.values[i],
            self.values[i + 1]
        )
    }
}

/// One cell's `[post_process]`, for a value on each axis it sits on: the
/// scene's, under a look axis's look (or the base look, without one), under
/// each post axis's field. Style and wooze axes leave it be.
fn cell_def(
    axes: &[(&Sweep, usize)],
    scene: Option<&PostProcessDef>,
    base: Option<&Look>,
    scene_path: &str,
) -> Result<Option<PostProcessDef>> {
    let mut def = match axes.iter().find(|(s, _)| s.key == SweepKey::Look) {
        Some((s, i)) => Some(
            find(&s.values[*i], scene_path)?
                .apply(scene)
                .map_err(|e| anyhow::anyhow!(e))?,
        ),
        None => base_def(scene, base)?,
    };
    for (s, i) in axes {
        if let SweepKey::Post { field } = &s.key {
            let value = &s.values[*i];
            let mut over = toml::Table::new();
            over.insert(field.clone(), parse_value(value));
            def = Some(
                overlay(def.as_ref(), &over)
                    .with_context(|| format!("--sweep post.{field}={value}"))?,
            );
        }
    }
    Ok(def)
}

/// Most cells a two-key sheet takes.
pub const MAX_CELLS: usize = 48;

/// One `--sweep` (a row of columns) or two (#358): the first across, the
/// second down, a cell for every pair, so two keys that interact (an ASCII
/// look's brightness against its contrast) are judged on one sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepGrid {
    /// Across: a column per value.
    pub columns: Sweep,
    /// Down: a row per value, for a second `--sweep`.
    pub rows: Option<Sweep>,
}

impl SweepGrid {
    /// The grid `--sweep` was given, or none without one. Two sweeps of one
    /// key, two looks or more than two keys are refused with why.
    pub fn new(sweeps: &[Sweep]) -> Result<Option<Self>, String> {
        match sweeps {
            [] => Ok(None),
            [one] => Ok(Some(Self {
                columns: one.clone(),
                rows: None,
            })),
            [across, down] => {
                if across.key == SweepKey::Look && down.key == SweepKey::Look {
                    return Err("two --sweep look=...: a sheet sweeps one list of looks".into());
                }
                if across.name() == down.name() {
                    return Err(format!(
                        "--sweep {} is given twice: put every value in one --sweep",
                        across.name()
                    ));
                }
                let cells = across.values.len() * down.values.len();
                if cells > MAX_CELLS {
                    return Err(format!(
                        "--sweep {} x {}: {cells} cells; a sheet takes at most {MAX_CELLS}",
                        across.name(),
                        down.name()
                    ));
                }
                Ok(Some(Self {
                    columns: across.clone(),
                    rows: Some(down.clone()),
                }))
            }
            _ => Err(format!(
                "{} --sweep flags: a sheet takes one (a row) or two (a grid, the first \
                 across and the second down)",
                sweeps.len()
            )),
        }
    }

    /// Columns and rows.
    pub fn shape(&self) -> (usize, usize) {
        (
            self.columns.values.len(),
            self.rows.as_ref().map_or(1, |r| r.values.len()),
        )
    }

    /// Every cell's (column, row), a row at a time.
    pub fn cells(&self) -> Vec<(usize, usize)> {
        let (columns, rows) = self.shape();
        (0..rows)
            .flat_map(|r| (0..columns).map(move |c| (c, r)))
            .collect()
    }

    pub fn axes(&self, column: usize, row: usize) -> Vec<(&Sweep, usize)> {
        let mut axes = vec![(&self.columns, column)];
        if let Some(rows) = &self.rows {
            axes.push((rows, row));
        }
        axes
    }

    /// The keys, `ascii.brightness x ascii.contrast` for a grid.
    pub fn name(&self) -> String {
        match &self.rows {
            Some(rows) => format!("{} x {}", self.columns.name(), rows.name()),
            None => self.columns.name(),
        }
    }

    /// The cell's `[post_process]` (see [`Sweep::column_def`]).
    pub fn cell_def(
        &self,
        column: usize,
        row: usize,
        scene: Option<&PostProcessDef>,
        base: Option<&Look>,
        scene_path: &str,
    ) -> Result<Option<PostProcessDef>> {
        cell_def(&self.axes(column, row), scene, base, scene_path)
    }

    /// The look the cell's `look=` axis names, if either axis is one.
    pub fn cell_look(&self, column: usize, row: usize) -> Option<&str> {
        self.axes(column, row)
            .into_iter()
            .find(|(s, _)| s.key == SweepKey::Look)
            .map(|(s, i)| s.values[i].as_str())
    }

    /// The cell's `--style` words, one per style axis.
    pub fn cell_styles(&self, column: usize, row: usize) -> Vec<String> {
        self.axes(column, row)
            .into_iter()
            .filter_map(|(s, i)| s.column_style(i))
            .collect()
    }

    /// What the cell was given, for the terminal: `ascii.cell = 8`, or both
    /// keys' values in a grid.
    pub fn describe(&self, column: usize, row: usize) -> String {
        self.axes(column, row)
            .into_iter()
            .map(|(s, i)| format!("{} = {}", s.name(), s.values[i]))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Whether either axis turns a field of the scene's `[living]` table.
    pub fn is_living(&self) -> bool {
        self.axes(0, 0)
            .iter()
            .any(|(s, _)| matches!(s.key, SweepKey::Echoes { .. }))
    }

    /// Write the cell's `[living]` values into a scene's tables (#531): each
    /// echoes axis's field into `[living] echoes`, the table and its `echoes`
    /// made when the scene has none, every other field left as the scene
    /// wrote it. The cell then draws what a scene with that table draws,
    /// since everything downstream reads `[living]` from these tables.
    pub fn lay_living(&self, column: usize, row: usize, tables: &mut toml::Table) {
        for (sweep, i) in self.axes(column, row) {
            let Some((knob, value)) = sweep.echoes_field(i) else {
                continue;
            };
            let living = tables
                .entry("living")
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            // A [living] that isn't a table was refused when the scene settled
            let Some(living) = living.as_table_mut() else {
                continue;
            };
            let echoes = living
                .entry("echoes")
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            if let Some(echoes) = echoes.as_table_mut() {
                echoes.insert(knob.to_string(), value);
            }
        }
    }

    /// The first wooze knob either axis turns.
    pub fn wooze_knob(&self) -> Option<&str> {
        self.columns
            .wooze_knob()
            .or_else(|| self.rows.as_ref().and_then(|r| r.wooze_knob()))
    }

    /// A warning for every pair of neighbours, across or down, that drew the
    /// same pixels in some row (or column). `frames` is a row at a time.
    pub fn identical_warnings<T: PartialEq>(&self, frames: &[T]) -> Vec<String> {
        let (columns, rows) = self.shape();
        let mut out = Vec::new();
        // `index(i, l)`: the frame of value `i` on line `l` of the other key
        let mut warn =
            |sweep: &Sweep, lines: usize, len: usize, index: &dyn Fn(usize, usize) -> usize| {
                for i in 0..len.saturating_sub(1) {
                    let same = (0..lines)
                        .filter(|&l| frames[index(i, l)] == frames[index(i + 1, l)])
                        .count();
                    if same == 0 {
                        continue;
                    }
                    let mut line = sweep.identical_warning(i);
                    if lines > 1 {
                        line.push_str(&format!(" (in {same} of {lines})"));
                    }
                    out.push(line);
                }
            };
        warn(&self.columns, rows, columns, &|i, l| l * columns + i);
        if let Some(down) = &self.rows {
            warn(down, columns, rows, &|i, l| i * columns + l);
        }
        out
    }
}

/// A full-size render's default, for cells that draw at the final scale.
pub const FULL_SIZE: (u32, u32) = (1920, 1080);

/// What a sheet's cell is of the frame it was drawn as (#358). A style sized
/// in points (ASCII's cells, pencil's spacing) draws a third as many glyphs
/// across a 640-wide cell as across a 1920-wide frame, so a small cell can
/// pick a winner the final render disagrees with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CellView {
    /// Each cell its own small frame (640x360 unless `--width/--height`):
    /// quick, and the whole composition, but at a small frame's scale.
    Small,
    /// Each drawn at the full size and a region kept pixel for pixel
    /// (`--sweep-crop X,Y,W,H`): glyphs true to size, a part of the frame.
    Crop(Crop),
    /// Each drawn at the full size and shrunk to a cell (`--sweep-scale
    /// full`): the whole composition the final render has, its glyphs blurred.
    Full,
}

/// The region a `--sweep-crop` keeps: in pixels of the frame, or as
/// fractions of it (#501), so one crop holds at any `--width/--height`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Crop {
    /// `700,380,480,270`: X, Y, W, H in pixels.
    Pixels([u32; 4]),
    /// `0.4,0.48,0.33,0.33`: X, Y, W, H as shares of the frame's width and
    /// height, each in 0..=1, the region inside the frame.
    Fraction([f64; 4]),
}

impl Crop {
    /// The region in pixels of a `width` x `height` frame. A fraction's
    /// edges are rounded, not its size, so a region that ends at the frame's
    /// edge ends there at any size; it keeps at least a pixel each way.
    pub fn pixels(&self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        match *self {
            Crop::Pixels([x, y, w, h]) => (x, y, w, h),
            Crop::Fraction([fx, fy, fw, fh]) => {
                let edge = |f: f64, size: u32| ((f * f64::from(size)).round() as u32).min(size);
                let (x0, y0) = (edge(fx, width), edge(fy, height));
                let (x1, y1) = (edge(fx + fw, width), edge(fy + fh, height));
                let x0 = x0.min(width.saturating_sub(1));
                let y0 = y0.min(height.saturating_sub(1));
                (x0, y0, x1.max(x0 + 1) - x0, y1.max(y0 + 1) - y0)
            }
        }
    }
}

/// How wide a `CellView::Full` cell is shrunk to.
pub const FULL_CELL_WIDTH: u32 = 640;

/// `clap`'s value parser for `--sweep-crop X,Y,W,H`: pixels of the full
/// frame, or, when any value has a decimal point, fractions of it (#501).
pub fn parse_crop(spec: &str) -> Result<CellView, String> {
    let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
    let usage = || {
        format!(
            "--sweep-crop takes X,Y,W,H in pixels of the full frame or as fractions of it \
             (got '{spec}'), e.g. 640,360,640,360 or 0.33,0.33,0.33,0.33 for the middle \
             of a 1920x1080 frame"
        )
    };
    if parts.iter().any(|p| p.contains('.')) {
        let numbers: Option<Vec<f64>> = parts.iter().map(|p| p.parse().ok()).collect();
        let Some(&[x, y, w, h]) = numbers.as_deref() else {
            return Err(usage());
        };
        const SLACK: f64 = 1e-9;
        if [x, y, w, h]
            .iter()
            .any(|v| !(v.is_finite() && (0.0..=1.0).contains(v)))
        {
            return Err(format!(
                "--sweep-crop {spec}: a crop in fractions takes each value from 0 to 1 \
                 (pixels have no decimal point: 700,380,480,270)"
            ));
        }
        if w <= 0.0 || h <= 0.0 {
            return Err(format!(
                "--sweep-crop {spec}: the width and height must be over 0"
            ));
        }
        if x + w > 1.0 + SLACK || y + h > 1.0 + SLACK {
            return Err(format!(
                "--sweep-crop {spec}: the region reaches past the frame (X + W and Y + H \
                 must be at most 1)"
            ));
        }
        return Ok(CellView::Crop(Crop::Fraction([x, y, w, h])));
    }
    let numbers: Option<Vec<u32>> = parts.iter().map(|p| p.parse().ok()).collect();
    match numbers.as_deref() {
        Some(&[x, y, w, h]) if w > 0 && h > 0 => Ok(CellView::Crop(Crop::Pixels([x, y, w, h]))),
        Some(&[_, _, _, _]) => Err(format!(
            "--sweep-crop {spec}: the width and height must be over 0"
        )),
        _ => Err(usage()),
    }
}

impl CellView {
    /// From `--sweep-crop` and `--sweep-scale`.
    pub fn from_args(crop: Option<CellView>, scale: Option<&str>) -> Self {
        match (crop, scale) {
            (Some(crop), _) => crop,
            (None, Some("full")) => CellView::Full,
            _ => CellView::Small,
        }
    }

    /// The size each cell is drawn at, before it's cropped or shrunk.
    pub fn render_size(&self, width: Option<u32>, height: Option<u32>) -> (u32, u32) {
        let (w, h) = match self {
            CellView::Small => CELL_SIZE,
            CellView::Crop(_) | CellView::Full => FULL_SIZE,
        };
        (width.unwrap_or(w), height.unwrap_or(h))
    }

    /// A crop that leaves the frame is refused before any drawing. A crop
    /// in fractions never does.
    pub fn check(&self, width: u32, height: u32) -> Result<(), String> {
        if let CellView::Crop(Crop::Pixels([x, y, w, h])) = *self {
            if x.saturating_add(w) > width || y.saturating_add(h) > height {
                return Err(format!(
                    "--sweep-crop {x},{y},{w},{h} reaches past the {width}x{height} frame \
                     (set --width/--height, a smaller crop, or give it in fractions)"
                ));
            }
        }
        Ok(())
    }

    /// The region a crop keeps of a `width` x `height` frame, in pixels;
    /// `None` for a cell that shows the whole frame.
    pub fn region(&self, width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
        match self {
            CellView::Crop(crop) => Some(crop.pixels(width, height)),
            CellView::Small | CellView::Full => None,
        }
    }

    /// The cell a drawn frame becomes.
    pub fn cell(&self, frame: image::RgbaImage) -> image::RgbaImage {
        let (fw, fh) = frame.dimensions();
        if let Some((x, y, w, h)) = self.region(fw, fh) {
            return image::imageops::crop_imm(&frame, x, y, w, h).to_image();
        }
        match *self {
            CellView::Full if fw > FULL_CELL_WIDTH => {
                let th = ((u64::from(fh) * u64::from(FULL_CELL_WIDTH) + u64::from(fw) / 2)
                    / u64::from(fw))
                .max(1) as u32;
                image::imageops::resize(
                    &frame,
                    FULL_CELL_WIDTH,
                    th,
                    image::imageops::FilterType::Triangle,
                )
            }
            _ => frame,
        }
    }

    /// Where a crop sheet's cells come from (#501): the whole of one drawn
    /// frame, with the region its cells keep. `None` unless this is a crop.
    /// A crop is the same region of every cell's frame, so the first one
    /// stands for them all.
    pub fn locator(&self, frame: &image::RgbaImage) -> Option<super::style_sheet::Locator> {
        let (w, h) = frame.dimensions();
        self.region(w, h).map(|region| super::style_sheet::Locator {
            frame: frame.clone(),
            region,
        })
    }

    /// What a reader has to know about the scale, for the sheet's subtitle.
    pub fn describe(&self, width: u32, height: u32) -> String {
        match *self {
            CellView::Small => format!("{width}x{height} frames"),
            CellView::Crop(crop) => {
                let (x, y, w, h) = crop.pixels(width, height);
                format!("1:1 {w}x{h} at {x},{y} of {width}x{height}")
            }
            CellView::Full => {
                let pct = (FULL_CELL_WIDTH.min(width) * 100 + width / 2) / width;
                format!("{width}x{height} shown at {pct}%")
            }
        }
    }
}

fn parse_post(field: &str, values: &[String]) -> Result<SweepKey, String> {
    if !PostProcessDef::field_names().iter().any(|k| k == field) {
        return Err(format!(
            "--sweep: [post_process] has no field '{field}'{}",
            did_you_mean(PostProcessDef::closest_field(field))
        ));
    }
    // A built-in effect's value past what it is tuned for is refused, as a
    // style sweep's is (#355, #741)
    if let Some(param) = flint_schema::post_effects::param(field) {
        for value in values {
            if let Some(e) = param.range_error(&parse_value(value)) {
                return Err(format!(
                    "--sweep post.{field}={value}: {e} (`flint render --list-effects` lists \
                     every key's range)"
                ));
            }
        }
    }
    Ok(SweepKey::Post {
        field: field.to_string(),
    })
}

/// A wooze knob's numbers are checked against its range by the field when the
/// column is drawn, which says the same thing a bad scene table does; here is
/// only what's needed to know what was meant.
fn parse_wooze(knob: &str, values: &[String]) -> Result<SweepKey, String> {
    if !WOOZE_KNOBS.contains(&knob) {
        return Err(format!(
            "--sweep: can't sweep 'wooze.{knob}': it takes wooze.{}",
            WOOZE_KNOBS.join(", wooze.")
        ));
    }
    if knob != "preset" {
        if let Some(wrong) = values.iter().find(|v| v.parse::<f32>().is_err()) {
            return Err(format!(
                "--sweep: wooze.{knob} takes numbers, and '{wrong}' isn't"
            ));
        }
    }
    Ok(SweepKey::Wooze {
        knob: knob.to_string(),
    })
}

/// `living.<field>`: `[living] echoes`' fields (#531), each value held to
/// what a spec's echoes take (`EchoConfig::check`), so no column draws a value
/// the props would refuse.
fn parse_living(field: &str, values: &[String]) -> Result<SweepKey, String> {
    let Some(knob) = field
        .strip_prefix("echoes.")
        .filter(|k| ECHO_KNOBS.contains(k))
    else {
        return Err(format!(
            "--sweep: can't sweep 'living.{field}': it takes living.echoes.{}",
            ECHO_KNOBS.join(", living.echoes.")
        ));
    };
    for value in values {
        let mut echoes = flint_procgen::EchoConfig::default();
        if knob == "count" {
            echoes.count = value.parse().map_err(|_| {
                format!("--sweep: living.echoes.count takes whole numbers, and '{value}' isn't")
            })?;
        } else {
            let number = value
                .parse::<f32>()
                .ok()
                .filter(|n| n.is_finite())
                .ok_or_else(|| {
                    format!("--sweep: living.echoes.{knob} takes numbers, and '{value}' isn't")
                })?;
            if knob == "spread" {
                echoes.spread = number;
            } else {
                echoes.alpha = number;
            }
        }
        // `why` starts with the value: `3: must be from 0 to 1.82 steps, ...`
        if let Err((_, why)) = echoes.check() {
            return Err(format!("--sweep living.echoes.{knob}={why}"));
        }
    }
    Ok(SweepKey::Echoes {
        knob: knob.to_string(),
    })
}

fn parse_style(key: &str, style: &str, k: &str, values: &[String]) -> Result<SweepKey, String> {
    for value in values {
        // `StyleSpec` refuses a value past its param's range (#355), which
        // would draw the same column twice
        let word = format!("{style}:{k}={value}");
        match flint_render::stylize::StyleSpec::parse(
            &word,
            &flint_render::stylize::StyleRegistry::new(),
        ) {
            Ok(_) => {}
            // Maybe a user style (#274), registered only once the scene's
            // folder is read; the column's apply checks it
            Err(e) if e.starts_with("unknown style") => {}
            Err(e) => return Err(format!("--sweep {key}={value}: {e}")),
        }
    }
    Ok(SweepKey::Style {
        style: style.to_string(),
        key: k.to_string(),
    })
}

/// The scene's `[post_process]` under the base look, if any.
pub fn base_def(
    scene: Option<&PostProcessDef>,
    look: Option<&Look>,
) -> Result<Option<PostProcessDef>> {
    match look {
        Some(look) => Ok(Some(look.apply(scene).map_err(|e| anyhow::anyhow!(e))?)),
        None => Ok(scene.cloned()),
    }
}

/// A sweep value as TOML: a number, bool or array when it reads as one
/// (`0.7,0.75,0.82` becomes an array), otherwise a string.
pub fn parse_value(text: &str) -> toml::Value {
    let try_parse = |t: &str| {
        toml::from_str::<toml::Table>(&format!("v = {t}"))
            .ok()
            .and_then(|mut table| table.remove("v"))
    };
    if text.contains(',') {
        if let Some(v) = try_parse(&format!("[{text}]")) {
            return v;
        }
    }
    try_parse(text).unwrap_or_else(|| toml::Value::String(text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweeps_parse_each_kind() {
        let s = Sweep::parse("ascii.cell=6,8,12").unwrap();
        assert_eq!(
            s.key,
            SweepKey::Style {
                style: "ascii".into(),
                key: "cell".into()
            }
        );
        assert_eq!(s.column_style(2).as_deref(), Some("ascii:cell=12"));
        assert_eq!(s.wooze_table(0), None, "a style column leaves [wooze] be");

        let s = Sweep::parse("look=ascii_noir,matrix").unwrap();
        assert_eq!(s.key, SweepKey::Look);

        let s = Sweep::parse("post.fog_color=0,0,0|0.7,0.75,0.82").unwrap();
        assert_eq!(s.values, ["0,0,0", "0.7,0.75,0.82"]);
        let def = s.column_def(1, None, None, "").unwrap().unwrap();
        assert_eq!(def.fog_color, [0.7, 0.75, 0.82]);
    }

    /// #353's done-when, at the parser: both families from the one flag.
    #[test]
    fn a_style_and_a_wooze_sweep_come_through_one_parser() {
        let style = parse_sweep("ascii.cell=6,8").unwrap();
        let wooze = parse_sweep("wooze.glide=0,1").unwrap();
        assert_eq!(style.name(), "ascii.cell");
        assert_eq!(wooze.name(), "wooze.glide");
        assert_eq!(wooze.wooze_knob(), Some("glide"));
        assert_eq!(wooze.column_style(0), None, "a wooze column sets no style");
        // ...and draws with the scene's own post-processing
        let scene: PostProcessDef = toml::from_str("exposure = 2.5").unwrap();
        let def = wooze
            .column_def(1, Some(&scene), None, "")
            .unwrap()
            .unwrap();
        assert_eq!(def.exposure, 2.5);
    }

    // #216's parser tests (feature/living-props' render_plan.rs), on the one type

    #[test]
    fn the_tickets_wooze_sweep_parses() {
        let sweep = parse_sweep("wooze.glide=0,0.25,0.5,1").unwrap();
        assert_eq!(sweep.name(), "wooze.glide");
        assert_eq!(sweep.values, ["0", "0.25", "0.5", "1"]);
        assert_eq!(
            sweep.wooze_table(1).unwrap()["glide"].as_float(),
            Some(0.25)
        );
        assert_eq!(sweep.label(1), "glide 0.25");
    }

    #[test]
    fn a_preset_sweep_keeps_its_names() {
        let sweep = parse_sweep("wooze.preset=boil,morph").unwrap();
        assert_eq!(
            sweep.wooze_table(1).unwrap()["preset"].as_str(),
            Some("morph")
        );
        assert_eq!(sweep.label(1), "morph", "a preset is labelled by its name");
    }

    #[test]
    fn a_bad_wooze_sweep_says_what_was_wrong() {
        assert!(parse_sweep("wooze.glide").unwrap_err().contains("KEY=V1"));
        let unknown = parse_sweep("wooze.wobble=1,2").unwrap_err();
        assert!(unknown.contains("wooze.wobble") && unknown.contains("wooze.glide"));
        assert!(parse_sweep("glide=1,2").is_err());
        assert!(parse_sweep("wooze.glide=0,,1")
            .unwrap_err()
            .contains("value 2"));
        assert!(parse_sweep("wooze.glide=0,dreamy")
            .unwrap_err()
            .contains("'dreamy'"));
    }

    /// #531: an echoes field parses, labels like a wooze knob and is written
    /// into the scene's `[living] echoes` as a scene would write it.
    #[test]
    fn an_echoes_sweep_is_written_into_the_living_table() {
        let spread = parse_sweep("living.echoes.spread=0,0.9,1.8").unwrap();
        assert_eq!(spread.name(), "living.echoes.spread");
        assert_eq!(spread.label(1), "spread 0.9");
        assert_eq!(spread.wooze_knob(), None, "no wooze knob");
        assert_eq!(spread.column_style(0), None, "and no style");
        let count = parse_sweep("living.echoes.count=0,3").unwrap();
        assert_eq!(
            count.echoes_field(1),
            Some(("count", toml::Value::Integer(3)))
        );

        // Over a scene's table: the column's field, the scene's others kept
        let grid = SweepGrid::new(&[spread]).unwrap().unwrap();
        assert!(grid.is_living());
        let mut tables: toml::Table = toml::from_str(
            "[living]\nstagger = \"none\"\nsoftness = 0.3\nechoes = { count = 2, spread = 0.2 }",
        )
        .unwrap();
        grid.lay_living(1, 0, &mut tables);
        let scene: toml::Table = toml::from_str(
            "[living]\nstagger = \"none\"\nsoftness = 0.3\nechoes = { count = 2, spread = 0.9 }",
        )
        .unwrap();
        assert_eq!(tables, scene, "what a scene with spread = 0.9 says");

        // A scene with no [living] gets one holding only the field
        let mut bare = toml::Table::new();
        grid.lay_living(2, 0, &mut bare);
        let expected: toml::Table = toml::from_str("[living]\nechoes = { spread = 1.8 }").unwrap();
        assert_eq!(bare, expected);

        // In a grid, each axis writes its own field
        let grid = SweepGrid::new(&[
            parse_sweep("living.echoes.alpha=0.1,0.4").unwrap(),
            parse_sweep("living.echoes.count=1,6").unwrap(),
        ])
        .unwrap()
        .unwrap();
        let mut both = toml::Table::new();
        grid.lay_living(1, 1, &mut both);
        let expected: toml::Table =
            toml::from_str("[living]\nechoes = { alpha = 0.4, count = 6 }").unwrap();
        assert_eq!(both, expected);
        assert!(!SweepGrid::new(&[parse_sweep("wooze.glide=0,1").unwrap()])
            .unwrap()
            .unwrap()
            .is_living());
    }

    #[test]
    fn a_bad_echoes_sweep_says_what_was_wrong() {
        let unknown = parse_sweep("living.echoes.width=1,2").unwrap_err();
        assert!(unknown.contains("living.echoes.spread"), "{unknown}");
        assert!(parse_sweep("living.softness=0,1")
            .unwrap_err()
            .contains("living.echoes.count"));
        assert!(parse_sweep("living.echoes.count=1,2.5")
            .unwrap_err()
            .contains("whole numbers"));
        assert!(parse_sweep("living.echoes.count=-1,2").is_err());
        let many = parse_sweep("living.echoes.count=1,40").unwrap_err();
        assert!(many.contains("at most 16"), "{many}");
        let wide = parse_sweep("living.echoes.spread=0,3").unwrap_err();
        assert!(wide.contains("faded to nothing"), "{wide}");
        let bright = parse_sweep("living.echoes.alpha=0.2,1.5").unwrap_err();
        assert!(bright.contains("[0.0, 1.0]"), "{bright}");
        assert!(parse_sweep("living.echoes.alpha=0.2,NaN")
            .unwrap_err()
            .contains("takes numbers"));
        let s = parse_sweep("living.echoes.spread=0.5,0.5").unwrap();
        assert!(s.identical_warning(0).contains("count of 0"));
    }

    #[test]
    fn bad_sweeps_say_why() {
        assert!(Sweep::parse("ascii.cell=8")
            .unwrap_err()
            .contains("two values"));
        assert!(Sweep::parse("ascii.cel=6,8").is_err());
        let range = Sweep::parse("ascii.cell=6,400").unwrap_err();
        assert!(range.contains("outside its range 4..32"), "{range}");
        assert!(Sweep::parse("post.exposur=1,2")
            .unwrap_err()
            .contains("exposure"));
        assert!(Sweep::parse("cell=6,8").is_err());
        let many = vec!["1"; MAX_COLUMNS + 1].join(",");
        assert!(Sweep::parse(&format!("ascii.cell={many}"))
            .unwrap_err()
            .contains("at most"));
    }

    #[test]
    fn a_post_sweep_layers_over_the_base_look() {
        let look = Look::parse(
            "t",
            "[post_process]\nexposure = 3.0\ndesaturate = 0.5",
            "test",
        )
        .unwrap();
        let s = Sweep::parse("post.exposure=0.5,1").unwrap();
        let def = s.column_def(0, None, Some(&look), "").unwrap().unwrap();
        assert_eq!(def.exposure, 0.5);
        assert_eq!(def.desaturate, 0.5, "the look's other fields stay");
    }

    #[test]
    fn identical_columns_are_found_and_named() {
        let s = Sweep::parse("ascii.cell=6,8").unwrap();
        let w = s.identical_warning(0);
        assert!(
            w.starts_with("ascii.cell=6 and 8 are pixel-identical"),
            "{w}"
        );
        let s = parse_sweep("wooze.glide=0.5,0.5").unwrap();
        assert!(s.identical_warning(0).contains("zone"));
    }

    /// #358's second done-when, at the parser: two keys make one grid, and a
    /// cell takes a value from each.
    #[test]
    fn two_sweeps_make_a_grid() {
        let across = parse_sweep("ascii.brightness=0,0.15,0.3").unwrap();
        let down = parse_sweep("ascii.contrast=1,2").unwrap();
        let grid = SweepGrid::new(&[across, down]).unwrap().unwrap();
        assert_eq!(grid.shape(), (3, 2));
        assert_eq!(grid.name(), "ascii.brightness x ascii.contrast");
        assert_eq!(
            grid.cells()[..4],
            [(0, 0), (1, 0), (2, 0), (0, 1)],
            "a row at a time"
        );
        assert_eq!(
            grid.cell_styles(2, 1),
            ["ascii:brightness=0.3", "ascii:contrast=2"]
        );
        assert_eq!(
            grid.describe(1, 1),
            "ascii.brightness = 0.15, ascii.contrast = 2"
        );
        assert!(SweepGrid::new(&[]).unwrap().is_none());
    }

    #[test]
    fn a_look_down_and_a_post_field_across_meet_in_each_cell() {
        let across = parse_sweep("post.exposure=0.5,2").unwrap();
        let down = parse_sweep("look=ascii_noir,graphite").unwrap();
        let grid = SweepGrid::new(&[across, down]).unwrap().unwrap();
        let noir = find("ascii_noir", "").unwrap().apply(None).unwrap();
        let graphite = find("graphite", "").unwrap().apply(None).unwrap();
        let cell = grid.cell_def(1, 0, None, None, "").unwrap().unwrap();
        assert_eq!(cell.exposure, 2.0, "the column's field");
        assert_eq!(
            cell.styles.keys().collect::<Vec<_>>(),
            noir.styles.keys().collect::<Vec<_>>()
        );
        let cell = grid.cell_def(0, 1, None, None, "").unwrap().unwrap();
        assert_eq!(cell.exposure, 0.5);
        assert_eq!(
            cell.styles.keys().collect::<Vec<_>>(),
            graphite.styles.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn grids_that_cant_be_drawn_say_why() {
        let s = |t: &str| parse_sweep(t).unwrap();
        let twice = SweepGrid::new(&[s("ascii.cell=6,8"), s("ascii.cell=10,12")]).unwrap_err();
        assert!(twice.contains("given twice"), "{twice}");
        let looks = SweepGrid::new(&[s("look=matrix,graphite"), s("look=ascii_noir,ink_wash")]);
        assert!(looks.unwrap_err().contains("one list of looks"));
        let three = SweepGrid::new(&[
            s("ascii.cell=6,8"),
            s("ascii.ink=1,2"),
            s("post.exposure=1,2"),
        ]);
        assert!(three.unwrap_err().contains("one (a row) or two"));
        // #741: a post value past its range is refused, naming it
        let hot = Sweep::parse("post.bloom_intensity=0.1,3").unwrap_err();
        assert!(
            hot.contains("bloom_intensity=3 is outside its range 0..1"),
            "{hot}"
        );
        let big = ["1"; 12].join(",");
        let many = SweepGrid::new(&[
            s(&format!("post.exposure={big}")),
            s(&format!("post.desaturate={}", ["0.5"; 5].join(","))),
        ]);
        assert!(many.unwrap_err().contains("at most 48"));
    }

    #[test]
    fn a_grid_warns_per_axis_and_counts_the_lines() {
        let s = |t: &str| parse_sweep(t).unwrap();
        let grid = SweepGrid::new(&[s("ascii.cell=6,8"), s("ascii.ink=1,2")])
            .unwrap()
            .unwrap();
        // Across: identical in row 0 only. Down: column 1 identical.
        let frames = [1, 1, 2, 1];
        let w = grid.identical_warnings(&frames);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].starts_with("ascii.cell=6 and 8") && w[0].ends_with("(in 1 of 2)"));
        assert!(w[1].starts_with("ascii.ink=1 and 2") && w[1].ends_with("(in 1 of 2)"));
        let row = SweepGrid::new(&[s("ascii.cell=6,8")]).unwrap().unwrap();
        assert!(
            !row.identical_warnings(&[3, 3])[0].contains("(in"),
            "a row says it plainly"
        );
    }

    /// #358's first done-when, on the CPU half: a crop keeps the frame's own
    /// pixels, a full cell is the whole frame shrunk.
    #[test]
    fn cells_crop_one_to_one_or_shrink_whole() {
        let mut frame = image::RgbaImage::new(1920, 1080);
        for (x, y, p) in frame.enumerate_pixels_mut() {
            *p = image::Rgba([(x % 256) as u8, (y % 256) as u8, 7, 255]);
        }
        let crop = parse_crop("640,360,320,180").unwrap();
        assert_eq!(crop.render_size(None, None), (1920, 1080));
        let cell = crop.cell(frame.clone());
        assert_eq!(cell.dimensions(), (320, 180));
        assert_eq!(
            cell.get_pixel(0, 0),
            frame.get_pixel(640, 360),
            "pixel for pixel"
        );
        assert_eq!(cell.get_pixel(319, 179), frame.get_pixel(959, 539));
        assert_eq!(
            crop.describe(1920, 1080),
            "1:1 320x180 at 640,360 of 1920x1080"
        );

        let full = CellView::from_args(None, Some("full"));
        assert_eq!(full.cell(frame).dimensions(), (640, 360));
        assert_eq!(full.describe(1920, 1080), "1920x1080 shown at 33%");

        let small = CellView::from_args(None, None);
        assert_eq!(small.render_size(None, Some(200)), (640, 200));
    }

    /// #501: a crop in fractions of the frame, the same region at any size.
    #[test]
    fn a_crop_in_fractions_scales_with_the_frame() {
        let crop = parse_crop("0.25,0.5,0.5,0.5").unwrap();
        assert_eq!(crop.region(1920, 1080), Some((480, 540, 960, 540)));
        assert_eq!(crop.region(640, 360), Some((160, 180, 320, 180)));
        assert_eq!(crop.describe(640, 360), "1:1 320x180 at 160,180 of 640x360");
        assert!(crop.check(17, 3).is_ok(), "a fraction is always inside");
        let frame = image::RgbaImage::new(1280, 720);
        assert_eq!(crop.cell(frame).dimensions(), (640, 360));
        // Edges round, not sizes: a region to the frame's edge ends there
        let right = parse_crop("0.5,0,0.5,1").unwrap();
        assert_eq!(right.region(101, 7), Some((51, 0, 50, 7)));
        // The golden pair: these fractions are 220,130,240,135 of 640x360
        assert_eq!(
            parse_crop("0.34375,0.3611,0.375,0.375")
                .unwrap()
                .region(640, 360),
            parse_crop("220,130,240,135").unwrap().region(640, 360),
        );
        // Never empty, even far smaller than a pixel
        assert_eq!(
            parse_crop("0.999,0.999,0.0001,0.0001")
                .unwrap()
                .region(100, 100),
            Some((99, 99, 1, 1))
        );
        // Any decimal point makes the whole crop fractional
        assert_eq!(
            parse_crop("0,0,1.0,1").unwrap(),
            CellView::Crop(Crop::Fraction([0.0, 0.0, 1.0, 1.0]))
        );
        assert_eq!(
            parse_crop("0,0,1,1").unwrap(),
            CellView::Crop(Crop::Pixels([0, 0, 1, 1]))
        );
    }

    #[test]
    fn crops_in_fractions_that_cant_be_are_refused() {
        assert!(parse_crop("700,0.5,0.3,0.3")
            .unwrap_err()
            .contains("from 0 to 1"));
        assert!(parse_crop("0.5,0.5,0,0.2").unwrap_err().contains("over 0"));
        assert!(parse_crop("0.6,0,0.5,0.5")
            .unwrap_err()
            .contains("past the frame"));
        assert!(parse_crop("0.1,0.1,0.5").is_err());
        assert!(parse_crop("0.1,NaN,0.5,0.5").is_err());
    }

    /// The locator is the first frame whole, with the region it keeps; only
    /// a crop has one.
    #[test]
    fn only_a_crop_has_a_locator() {
        let frame = image::RgbaImage::new(640, 360);
        let crop = parse_crop("0.5,0.5,0.25,0.25").unwrap();
        let locator = crop.locator(&frame).unwrap();
        assert_eq!(locator.region, (320, 180, 160, 90));
        assert_eq!(locator.frame.dimensions(), (640, 360));
        assert!(CellView::Full.locator(&frame).is_none());
        assert!(CellView::Small.locator(&frame).is_none());
    }

    #[test]
    fn crops_that_leave_the_frame_are_refused() {
        let crop = parse_crop("1600,0,640,360").unwrap();
        assert!(crop.check(1920, 1080).unwrap_err().contains("reaches past"));
        assert!(crop.check(2560, 1440).is_ok());
        assert!(parse_crop("1,2,3").is_err());
        assert!(parse_crop("0,0,0,10").unwrap_err().contains("over 0"));
        assert!(parse_crop("a,0,1,1").is_err());
    }

    #[test]
    fn style_labels_are_the_values() {
        let s = Sweep::parse("look=ascii_noir,matrix").unwrap();
        assert_eq!(s.label(1), "matrix");
    }
}
