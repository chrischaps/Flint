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
//! ```
//!
//! What a key means is decided here, before a scene loads or a GPU wakes, so a
//! typo costs nothing. How a column takes its value is up to its family: a look,
//! post or style column changes the `[post_process]` it draws with
//! ([`Sweep::column_def`], [`Sweep::column_style`]); a wooze column lays
//! [`Sweep::wooze_table`] over the scene's `[wooze]`, which only a build with the
//! wooze field can draw (feature/living-props, #206). The sheet the columns land
//! on is [`super::style_sheet::compose`] for every family.

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
                "expected KEY=V1,V2,... (got '{spec}'): e.g. ascii.cell=6,8,12 or \
                 wooze.glide=0,0.25,0.5,1"
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
            parse_post(field)?
        } else if let Some(knob) = key.strip_prefix("wooze.") {
            parse_wooze(knob, &values)?
        } else if let Some((style, k)) = key.split_once('.') {
            parse_style(key, style, k, &values)?
        } else {
            return Err(format!(
                "--sweep: '{key}' is not look, post.<field>, <style>.<key> or wooze.<knob> \
                 (e.g. ascii.cell; `--style help` lists the keys)"
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
        }
    }

    /// What column `i` is labelled with on the sheet: its value, or for a wooze
    /// number the knob and its value (`glide 0.25`), as #216's sheets were.
    pub fn label(&self, i: usize) -> String {
        let value = &self.values[i];
        match &self.key {
            SweepKey::Wooze { knob } if knob != "preset" => format!("{knob} {value}"),
            _ => value.clone(),
        }
    }

    /// Column `i`'s `[post_process]`: the scene's, under the base look (or
    /// the column's own, for a look sweep), under a post field's value. A
    /// style or wooze column draws with the base's. Kept for one-key callers
    /// (feature/living-props' wooze stages, #487); `flint render` asks
    /// [`SweepGrid::cell_def`].
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
    /// Read by the wooze stages on feature/living-props, which replace its own
    /// `render_plan::Sweep::table` with this when it merges.
    #[cfg_attr(not(test), allow(dead_code))]
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

    /// Said when columns `i` and `i + 1` came out the same, pixel for pixel: the
    /// sweep showed nothing, and a reader would otherwise think it showed "no
    /// difference". What usually explains it depends on what was swept.
    pub fn identical_warning(&self, i: usize) -> String {
        let why = match &self.key {
            SweepKey::Wooze { .. } => {
                "nothing on screen is reading this knob at this moment, or a zone is keeping \
                 it off these values"
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

    fn axes(&self, column: usize, row: usize) -> Vec<(&Sweep, usize)> {
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
    Crop { x: u32, y: u32, w: u32, h: u32 },
    /// Each drawn at the full size and shrunk to a cell (`--sweep-scale
    /// full`): the whole composition the final render has, its glyphs blurred.
    Full,
}

/// How wide a `CellView::Full` cell is shrunk to.
pub const FULL_CELL_WIDTH: u32 = 640;

/// `clap`'s value parser for `--sweep-crop X,Y,W,H`.
pub fn parse_crop(spec: &str) -> Result<CellView, String> {
    let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
    let numbers: Option<Vec<u32>> = parts.iter().map(|p| p.parse().ok()).collect();
    match numbers.as_deref() {
        Some(&[x, y, w, h]) if w > 0 && h > 0 => Ok(CellView::Crop { x, y, w, h }),
        Some(&[_, _, _, _]) => Err(format!(
            "--sweep-crop {spec}: the width and height must be over 0"
        )),
        _ => Err(format!(
            "--sweep-crop takes X,Y,W,H in pixels of the full frame (got '{spec}'), \
             e.g. 640,360,640,360 for the middle of a 1920x1080 frame"
        )),
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
            CellView::Crop { .. } | CellView::Full => FULL_SIZE,
        };
        (width.unwrap_or(w), height.unwrap_or(h))
    }

    /// A crop that leaves the frame is refused before any drawing.
    pub fn check(&self, width: u32, height: u32) -> Result<(), String> {
        if let CellView::Crop { x, y, w, h } = *self {
            if x.saturating_add(w) > width || y.saturating_add(h) > height {
                return Err(format!(
                    "--sweep-crop {x},{y},{w},{h} reaches past the {width}x{height} frame \
                     (set --width/--height, or a smaller crop)"
                ));
            }
        }
        Ok(())
    }

    /// The cell a drawn frame becomes.
    pub fn cell(&self, frame: image::RgbaImage) -> image::RgbaImage {
        match *self {
            CellView::Small => frame,
            CellView::Crop { x, y, w, h } => {
                image::imageops::crop_imm(&frame, x, y, w, h).to_image()
            }
            CellView::Full => {
                let (w, h) = frame.dimensions();
                if w <= FULL_CELL_WIDTH {
                    return frame;
                }
                let th = ((u64::from(h) * u64::from(FULL_CELL_WIDTH) + u64::from(w) / 2)
                    / u64::from(w))
                .max(1) as u32;
                image::imageops::resize(
                    &frame,
                    FULL_CELL_WIDTH,
                    th,
                    image::imageops::FilterType::Triangle,
                )
            }
        }
    }

    /// What a reader has to know about the scale, for the sheet's subtitle.
    pub fn describe(&self, width: u32, height: u32) -> String {
        match *self {
            CellView::Small => format!("{width}x{height} frames"),
            CellView::Crop { x, y, w, h } => {
                format!("1:1 {w}x{h} at {x},{y} of {width}x{height}")
            }
            CellView::Full => {
                let pct = (FULL_CELL_WIDTH.min(width) * 100 + width / 2) / width;
                format!("{width}x{height} shown at {pct}%")
            }
        }
    }
}

fn parse_post(field: &str) -> Result<SweepKey, String> {
    if !PostProcessDef::field_names().iter().any(|k| k == field) {
        return Err(format!(
            "--sweep: [post_process] has no field '{field}'{}",
            did_you_mean(PostProcessDef::closest_field(field))
        ));
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

fn parse_style(key: &str, style: &str, k: &str, values: &[String]) -> Result<SweepKey, String> {
    for value in values {
        // `StyleSpec` refuses a value past its param's range (#355), which
        // would draw the same column twice
        match flint_render::stylize::StyleSpec::parse(&format!("{style}:{k}={value}")) {
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
fn parse_value(text: &str) -> toml::Value {
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
