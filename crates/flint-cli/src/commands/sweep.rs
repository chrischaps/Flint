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
    /// style or wooze column draws with the base's.
    pub fn column_def(
        &self,
        i: usize,
        scene: Option<&PostProcessDef>,
        base: Option<&Look>,
        scene_path: &str,
    ) -> Result<Option<PostProcessDef>> {
        let value = &self.values[i];
        match &self.key {
            SweepKey::Look => Ok(Some(
                find(value, scene_path)?
                    .apply(scene)
                    .map_err(|e| anyhow::anyhow!(e))?,
            )),
            SweepKey::Style { .. } | SweepKey::Wooze { .. } => base_def(scene, base),
            SweepKey::Post { field } => {
                let based = base_def(scene, base)?;
                let mut over = toml::Table::new();
                over.insert(field.clone(), parse_value(value));
                Ok(Some(overlay(based.as_ref(), &over).with_context(|| {
                    format!("--sweep post.{field}={value}")
                })?))
            }
        }
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

/// The index of every column that drew exactly what the one after it did.
pub fn identical_neighbours<T: PartialEq>(cells: &[T]) -> Vec<usize> {
    cells
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0] == pair[1])
        .map(|(i, _)| i)
        .collect()
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
        match flint_render::stylize::StyleSpec::parse(&format!("{style}:{k}={value}")) {
            Ok(_) => {}
            // Maybe a user style (#274), registered only once the scene's
            // folder is read; the column's apply checks it
            Err(e) if e.starts_with("unknown style") => {}
            Err(e) => return Err(format!("--sweep {key}={value}: {e}")),
        }
        // A style clamps a param to its range, so a value past it would
        // draw the same column twice; say so instead.
        let param =
            flint_render::stylize::find(style).and_then(|d| d.params.iter().find(|p| p.name == k));
        if let (Some(p), Ok(n)) = (param, value.parse::<f32>()) {
            if n < p.min || n > p.max {
                return Err(format!(
                    "--sweep {key}={value}: outside {style}.{k}'s range {}..{}",
                    p.min, p.max
                ));
            }
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
        assert!(Sweep::parse("ascii.cell=6,400").is_err(), "out of range");
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
        assert_eq!(identical_neighbours(&[1, 1, 2, 3, 3]), [0, 3]);
        assert!(identical_neighbours::<u8>(&[]).is_empty());
        let s = Sweep::parse("ascii.cell=6,8").unwrap();
        let w = s.identical_warning(0);
        assert!(
            w.starts_with("ascii.cell=6 and 8 are pixel-identical"),
            "{w}"
        );
        let s = parse_sweep("wooze.glide=0.5,0.5").unwrap();
        assert!(s.identical_warning(0).contains("zone"));
    }

    #[test]
    fn style_labels_are_the_values() {
        let s = Sweep::parse("look=ascii_noir,matrix").unwrap();
        assert_eq!(s.label(1), "matrix");
    }
}
