//! Looks (#279): named post-processing presets an agent can put over any
//! scene. Sweeps of one setting at several values live in `sweep.rs`.
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

use anyhow::{anyhow, bail, Context, Result};
use flint_scene::PostProcessDef;
use std::path::{Path, PathBuf};

/// The engine's looks, compiled in so `--look` works from a game repo or an
/// installed binary as well as from the engine checkout.
const BUILTIN: &[(&str, &str)] = &[
    (
        "ascii_noir",
        include_str!("../../../../styles/ascii_noir.look.toml"),
    ),
    (
        "matrix",
        include_str!("../../../../styles/matrix.look.toml"),
    ),
    (
        "ink_wash",
        include_str!("../../../../styles/ink_wash.look.toml"),
    ),
    (
        "graphite",
        include_str!("../../../../styles/graphite.look.toml"),
    ),
    (
        "watercolor",
        include_str!("../../../../styles/watercolor.look.toml"),
    ),
];

/// The names of the looks compiled in.
#[cfg(test)]
pub fn builtin_names() -> impl Iterator<Item = &'static str> {
    BUILTIN.iter().map(|(name, _)| *name)
}

/// A look as written.
#[derive(Debug, Clone)]
pub struct Look {
    pub name: String,
    pub description: String,
    /// Sweeps worth trying first, in `--sweep` syntax.
    pub tune: Vec<String>,
    /// Advice for whoever tunes it (an agent, mostly).
    pub notes: String,
    /// Where it came from: a path, or `built-in`.
    pub source: String,
    /// The `[post_process]` block to merge.
    pub post_process: toml::Table,
}

impl Look {
    /// Parse a look's text; `name` is used when `[look] name` is absent.
    pub fn parse(name: &str, text: &str, source: &str) -> Result<Self> {
        let mut root: toml::Table =
            toml::from_str(text).with_context(|| format!("look '{name}' ({source})"))?;
        let meta = match root.remove("look") {
            Some(toml::Value::Table(t)) => t,
            Some(_) => bail!("look '{name}' ({source}): [look] must be a table"),
            None => toml::Table::new(),
        };
        let post_process = match root.remove("post_process") {
            Some(toml::Value::Table(t)) => t,
            Some(_) => bail!("look '{name}' ({source}): [post_process] must be a table"),
            None => bail!("look '{name}' ({source}) has no [post_process] block"),
        };
        if let Some(key) = root.keys().next() {
            bail!("look '{name}' ({source}): unknown top-level table [{key}] (a look has [look] and [post_process])");
        }
        let text_of = |key: &str| {
            meta.get(key)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let tune = meta
            .get("tune")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let look = Self {
            name: meta
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(name)
                .to_string(),
            description: text_of("description"),
            notes: text_of("notes"),
            tune,
            source: source.to_string(),
            post_process,
        };
        // Catch a misspelt field or style now, not as a silent default later.
        look.apply(None)
            .with_context(|| format!("look '{}' ({source})", look.name))?;
        Ok(look)
    }

    /// The scene's `[post_process]` with this look merged over it.
    pub fn apply(&self, scene: Option<&PostProcessDef>) -> Result<PostProcessDef> {
        overlay(scene, &self.post_process)
    }
}

/// `base` with `over` merged into it, checked by deserializing the result.
/// Keys `PostProcessDef` doesn't have are refused, so a typo can't vanish.
pub fn overlay(base: Option<&PostProcessDef>, over: &toml::Table) -> Result<PostProcessDef> {
    let mut table = match base {
        Some(def) => {
            toml::Table::try_from(def).context("serializing the scene's [post_process]")?
        }
        None => toml::Table::new(),
    };
    // Both sides in the style-table spelling first (#298), so a look's
    // `[styles.kuwahara] radius` overrides a scene's `kuwahara_radius`
    // and the other way round
    flint_scene::fold_legacy_style_keys(&mut table);
    let mut over = over.clone();
    flint_scene::fold_legacy_style_keys(&mut over);
    let over = &over;
    let known = known_fields();
    for key in over.keys() {
        if !known.iter().any(|k| k == key) {
            bail!(
                "[post_process] has no field '{key}'{}",
                suggestion(key, &known)
            );
        }
    }
    merge(&mut table, over);
    let def: PostProcessDef = toml::Value::Table(table)
        .try_into()
        .map_err(|e| anyhow!("[post_process]: {e}"))?;
    // The style tables are checked against the registry, as a scene's are.
    let (_, warnings) = flint_render::stylize::StyleSettings::from_tables(&def.styles);
    if let Some(first) = warnings.first() {
        bail!("{first}");
    }
    Ok(def)
}

/// Every field a `[post_process]` block can name.
pub(super) fn known_fields() -> Vec<String> {
    let def: PostProcessDef = toml::Value::Table(toml::Table::new())
        .try_into()
        .expect("an empty [post_process] deserializes");
    let mut fields: Vec<String> = toml::Table::try_from(&def)
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();
    // Skipped when empty, so the empty block doesn't show it
    fields.push("styles".to_string());
    fields
}

/// ", did you mean 'x'?" when one known name is close.
pub(super) fn suggestion(key: &str, known: &[String]) -> String {
    known
        .iter()
        .map(|k| (strsim_distance(key, k), k))
        .filter(|(d, _)| *d <= 3)
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| format!(", did you mean '{k}'?"))
        .unwrap_or_default()
}

/// Levenshtein distance, small and good enough for field names.
fn strsim_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = if ca == *cb {
                prev
            } else {
                1 + prev.min(row[j]).min(row[j + 1])
            };
            prev = cur;
        }
    }
    row[b.len()]
}

/// Merge `over` into `base`: tables recurse, everything else replaces.
pub fn merge(base: &mut toml::Table, over: &toml::Table) {
    for (key, value) in over {
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// The folders searched for `<name>.look.toml`, nearest first: the scene's
/// `styles/`, its parent's (a game's `levels/` sits beside `styles/`), and
/// the working directory's. A file found there wins over a built-in look of
/// the same name.
fn search_dirs(scene: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = Path::new(scene).parent() {
        dirs.push(dir.join("styles"));
        if let Some(parent) = dir.parent() {
            dirs.push(parent.join("styles"));
        }
    }
    dirs.push(PathBuf::from("styles"));
    dirs
}

/// Find a look by name or path.
pub fn find(name_or_path: &str, scene: &str) -> Result<Look> {
    let as_path = Path::new(name_or_path);
    if name_or_path.ends_with(".toml") {
        let text = std::fs::read_to_string(as_path)
            .with_context(|| format!("reading look {}", as_path.display()))?;
        let stem = file_stem(as_path);
        return Look::parse(&stem, &text, &as_path.display().to_string());
    }
    for dir in search_dirs(scene) {
        let path = dir.join(format!("{name_or_path}.look.toml"));
        if path.is_file() {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading look {}", path.display()))?;
            return Look::parse(name_or_path, &text, &path.display().to_string());
        }
    }
    if let Some((name, text)) = BUILTIN.iter().find(|(n, _)| *n == name_or_path) {
        return Look::parse(name, text, "built-in");
    }
    let names: Vec<String> = list(scene).into_iter().map(|l| l.name).collect();
    bail!(
        "unknown look '{name_or_path}'; known: {} (`--look help` describes them)",
        names.join(", ")
    )
}

fn file_stem(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.strip_suffix(".look.toml")
        .or_else(|| name.strip_suffix(".toml"))
        .unwrap_or(&name)
        .to_string()
}

/// Every look reachable from `scene`: the files on disk, then the built-ins
/// they don't shadow, sorted by name. Files that don't parse are skipped
/// with a warning.
pub fn list(scene: &str) -> Vec<Look> {
    let mut looks: Vec<Look> = Vec::new();
    for dir in search_dirs(scene) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with(".look.toml"))
            .collect();
        paths.sort();
        for path in paths {
            let stem = file_stem(&path);
            if looks.iter().any(|l| l.name == stem) {
                continue;
            }
            match std::fs::read_to_string(&path)
                .map_err(anyhow::Error::from)
                .and_then(|t| Look::parse(&stem, &t, &path.display().to_string()))
            {
                Ok(look) => looks.push(look),
                Err(e) => eprintln!("warning: skipping {}: {e:#}", path.display()),
            }
        }
    }
    for (name, text) in BUILTIN {
        if !looks.iter().any(|l| l.name == *name) {
            if let Ok(look) = Look::parse(name, text, "built-in") {
                looks.push(look);
            }
        }
    }
    looks.sort_by(|a, b| a.name.cmp(&b.name));
    looks
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
         field as post.<field>, look=a,b,c, or a Living Props knob as wooze.<knob> \
         (in builds with the wooze field).\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::sweep::Sweep;

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
}
