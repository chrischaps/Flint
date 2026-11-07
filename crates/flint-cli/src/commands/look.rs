//! Looks (#279): named post-processing presets an agent can put over any
//! scene, and sweeps that render one setting at several values side by side.
//!
//! A look is a `styles/<name>.look.toml`, beside the `*.style.toml`
//! generation guides: a `[look]` table that says what it is and what to tune,
//! and a `[post_process]` block in exactly the scene format. The block is
//! merged into the scene's own `[post_process]` key by key (tables merge,
//! everything else replaces), so a look changes only what it names.
//!
//! ```text
//! flint render demo/pbr_test.scene.toml --look ascii_noir -o noir.png
//! flint render demo/pbr_test.scene.toml --look ascii_noir --sweep ascii.cell=6,9,14 -o cells.png
//! flint render demo/pbr_test.scene.toml --sweep look=ascii_noir,matrix,ink_wash,graphite -o looks.png
//! flint render --look help
//! ```

use anyhow::{Context, Result};
use flint_scene::PostProcessDef;

// The looks themselves live in flint-render, where `flint-player --look`
// (#435) reads them too; the sweeps and `--look help` stay here.
#[cfg(test)]
pub use flint_render::look::builtin_names;
#[cfg(test)]
use flint_render::look::{file_stem, merge, BUILTIN};
use flint_render::look::{known_fields, suggestion};
pub use flint_render::look::{list, Look};
#[cfg(test)]
use std::path::Path;

/// Find a look by name or path.
pub fn find(name_or_path: &str, scene: &str) -> Result<Look> {
    Ok(flint_render::look::find(name_or_path, scene)?)
}

/// `base` with `over` merged into it, checked; see
/// [`flint_render::look::overlay`].
pub fn overlay(base: Option<&PostProcessDef>, over: &toml::Table) -> Result<PostProcessDef> {
    Ok(flint_render::look::overlay(base, over)?)
}

/// `--look help`: each look, what it is and what to sweep first.
pub fn help(scene: &str) -> String {
    let mut out = String::from(
        "Looks (styles/<name>.look.toml; a file beside the scene wins over a built-in):\n",
    );
    for look in list(scene) {
        out.push_str(&format!("\n  {}  [{}]\n", look.name, look.source));
        if !look.description.is_empty() {
            out.push_str(&format!("    {}\n", look.description));
        }
        for sweep in &look.tune {
            out.push_str(&format!("    tune: --sweep {sweep}\n"));
        }
        for line in look.notes.lines().filter(|l| !l.trim().is_empty()) {
            out.push_str(&format!("    | {}\n", line.trim()));
        }
    }
    out.push_str(
        "\nSweep any registered style key (`--style help` lists them), any [post_process] \
         field as post.<field>, or look=a,b,c.\n",
    );
    out
}

// --- Sweeps ----------------------------------------------------------------

/// What a sweep varies.
#[derive(Debug, Clone, PartialEq)]
pub enum SweepKey {
    /// `look=a,b,c`: each column a different look over the scene.
    Look,
    /// `<style>.<key>=...`: a registered style's param or option.
    Style { style: String, key: String },
    /// `post.<field>=...`: a `[post_process]` field.
    Post { field: String },
}

/// `--sweep KEY=V1,V2,...`, parsed and checked.
#[derive(Debug, Clone)]
pub struct Sweep {
    pub key: SweepKey,
    pub values: Vec<String>,
}

/// Most columns a sheet takes; past this the cells get too small to judge.
pub const MAX_COLUMNS: usize = 12;

impl Sweep {
    /// Parse `key=v1,v2,...`. Values split on `|` instead when one appears,
    /// so a vector can be swept: `post.fog_color=0,0,0|0.7,0.75,0.82`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let (key, values) = spec
            .split_once('=')
            .ok_or_else(|| format!("expected KEY=V1,V2,... (got '{spec}')"))?;
        let key = key.trim();
        let separator = if values.contains('|') { '|' } else { ',' };
        let values: Vec<String> = values
            .split(separator)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect();
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
            if !known_fields().iter().any(|k| k == field) {
                return Err(format!(
                    "--sweep: [post_process] has no field '{field}'{}",
                    suggestion(field, &known_fields())
                ));
            }
            SweepKey::Post {
                field: field.to_string(),
            }
        } else if let Some((style, k)) = key.split_once('.') {
            for value in &values {
                match flint_render::stylize::StyleSpec::parse(&format!("{style}:{k}={value}")) {
                    Ok(_) => {}
                    // Maybe a user style (#274), registered only once the
                    // scene's folder is read; the column's apply checks it
                    Err(e) if e.starts_with("unknown style") => {}
                    Err(e) => return Err(format!("--sweep {key}={value}: {e}")),
                }
                // A style clamps a param to its range, so a value past it
                // would draw the same column twice; say so instead.
                let param = flint_render::stylize::find(style)
                    .and_then(|d| d.params.iter().find(|p| p.name == k));
                if let (Some(p), Ok(n)) = (param, value.parse::<f32>()) {
                    if n < p.min || n > p.max {
                        return Err(format!(
                            "--sweep {key}={value}: outside {style}.{k}'s range {}..{}",
                            p.min, p.max
                        ));
                    }
                }
            }
            SweepKey::Style {
                style: style.to_string(),
                key: k.to_string(),
            }
        } else {
            return Err(format!(
                "--sweep: '{key}' is not look, post.<field> or <style>.<key> \
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
        }
    }

    /// Column `i`'s `[post_process]`: the scene's, under the base look (or
    /// the column's own, for a look sweep), under a post field's value.
    pub fn column_def(
        &self,
        i: usize,
        scene: Option<&PostProcessDef>,
        base: Option<&Look>,
        scene_path: &str,
    ) -> Result<Option<PostProcessDef>> {
        let value = &self.values[i];
        match &self.key {
            SweepKey::Look => Ok(Some(find(value, scene_path)?.apply(scene)?)),
            SweepKey::Style { .. } => base_def(scene, base),
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
}

/// The scene's `[post_process]` under the base look, if any.
pub fn base_def(
    scene: Option<&PostProcessDef>,
    look: Option<&Look>,
) -> Result<Option<PostProcessDef>> {
    match look {
        Some(look) => Ok(Some(look.apply(scene)?)),
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
    fn every_builtin_look_parses_and_checks() {
        for (name, text) in BUILTIN {
            let look =
                Look::parse(name, text, "built-in").unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert_eq!(look.name, *name, "[look] name matches the file");
            assert!(!look.description.is_empty(), "{name} says what it is");
            assert!(!look.tune.is_empty(), "{name} suggests a sweep");
            for sweep in &look.tune {
                Sweep::parse(sweep).unwrap_or_else(|e| panic!("{name} tune '{sweep}': {e}"));
            }
        }
    }

    #[test]
    fn the_shipped_files_are_the_builtins() {
        // A new styles/*.look.toml is compiled in, or `--look` from a game
        // repo can't find it.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../styles");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(".look.toml"))
            .map(|p| file_stem(&p))
            .collect();
        on_disk.sort();
        let mut builtin: Vec<String> = BUILTIN.iter().map(|(n, _)| n.to_string()).collect();
        builtin.sort();
        assert_eq!(on_disk, builtin);
    }

    #[test]
    fn a_look_changes_only_what_it_names() {
        let scene: PostProcessDef =
            toml::from_str("exposure = 2.5\nbloom_intensity = 0.3").unwrap();
        let look = Look::parse(
            "t",
            "[post_process]\nbloom_intensity = 0.1\n[post_process.styles.invert]\nenabled = true",
            "test",
        )
        .unwrap();
        let merged = look.apply(Some(&scene)).unwrap();
        assert_eq!(merged.exposure, 2.5, "the scene's exposure survives");
        assert_eq!(merged.bloom_intensity, 0.1, "the look's bloom wins");
        assert!(merged.styles.contains_key("invert"));
    }

    #[test]
    fn style_tables_merge_key_by_key() {
        let scene: PostProcessDef =
            toml::from_str("[styles.ascii]\nenabled = true\ncell = 12\nramp = \"blocks\"").unwrap();
        let mut over = toml::Table::new();
        merge(
            &mut over,
            &toml::from_str("[styles.ascii]\ncolor = \"mono\"").unwrap(),
        );
        let merged = overlay(Some(&scene), &over).unwrap();
        let ascii = &merged.styles["ascii"];
        assert_eq!(ascii["cell"].as_integer(), Some(12));
        assert_eq!(ascii["color"].as_str(), Some("mono"));
    }

    #[test]
    fn a_misspelt_field_is_refused_with_a_hint() {
        let err = Look::parse("t", "[post_process]\nbloom_intensty = 0.1", "test").unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("bloom_intensity"), "{text}");
    }

    #[test]
    fn an_unknown_style_is_refused() {
        let err = Look::parse(
            "t",
            "[post_process.styles.watercolour]\nenabled = true",
            "test",
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("watercolour"));
    }

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

        let s = Sweep::parse("look=ascii_noir,matrix").unwrap();
        assert_eq!(s.key, SweepKey::Look);

        let s = Sweep::parse("post.fog_color=0,0,0|0.7,0.75,0.82").unwrap();
        assert_eq!(s.values, ["0,0,0", "0.7,0.75,0.82"]);
        let def = s.column_def(1, None, None, "").unwrap().unwrap();
        assert_eq!(def.fog_color, [0.7, 0.75, 0.82]);
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
}
