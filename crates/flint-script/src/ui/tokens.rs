//! Style tokens — named values shared between `.style.toml` files and scripts.
//!
//! A style file may carry a local `[tokens]` table, or name a shared token
//! file with a top-level `tokens = "ui/theme.toml"`. To combine both (TOML
//! cannot hold a string and a table under one key) write `tokens_file =
//! "ui/theme.toml"` at the top level or `import = "ui/theme.toml"` inside
//! `[tokens]`. The shared file
//! is a set of sections (`[color]`, `[font]`, `[type]`, `[shape]`, `[space]`,
//! `[motion]`, ...). Any style property whose value is a string starting
//! with `$` is looked up here at parse time (and again when a script passes
//! a `"$name"` to `ui_set_style`).
//!
//! Lookup order for `"$name"`: the local `[tokens]` table first, then every
//! section of the shared file. `"$section.name"` addresses one shared
//! section explicitly. A local token may itself be a `"$ref"` into the
//! shared file (one level of indirection).

use super::element::StyleValue;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Key inside `[tokens]` that names the shared token file.
pub const IMPORT_KEY: &str = "import";

type Table = toml::map::Map<String, toml::Value>;
/// Shared token file sections, in file order.
type Sections = Vec<(String, Table)>;

/// Resolved token tables for one document.
#[derive(Debug, Clone, Default)]
pub struct TokenSet {
    /// `[tokens]` from the style file itself
    local: Table,
    /// Sections of the shared token file, in file order
    shared: Sections,
    /// Where the shared file was loaded from (for hot reload)
    pub shared_path: Option<PathBuf>,
    /// Token names already reported as unknown (warn once)
    warned: HashSet<String>,
}

impl TokenSet {
    /// Build from a parsed style document. `root_dir` is the project root
    /// (where `ui/theme.toml`-style paths resolve); `style_dir` is the
    /// fallback for paths relative to the style file itself.
    pub fn from_style_table(table: &Table, root_dir: &Path, style_dir: &Path) -> Self {
        let mut set = TokenSet::default();

        // TOML cannot hold both `tokens = "path"` and a `[tokens]` table, so
        // the shared file may also be named by top-level `tokens_file` or by
        // `import` inside the table.
        let mut shared_rel: Option<String> = None;
        if let Some(toml::Value::Table(t)) = table.get("tokens") {
            set.local = t.clone();
            if let Some(toml::Value::String(rel)) = set.local.remove(IMPORT_KEY) {
                shared_rel = Some(rel);
            }
        }
        if let Some(toml::Value::String(rel)) = table.get("tokens") {
            shared_rel = Some(rel.clone());
        }
        if let Some(toml::Value::String(rel)) = table.get("tokens_file") {
            shared_rel = Some(rel.clone());
        }

        if let Some(rel) = shared_rel.as_deref() {
            let candidates = [root_dir.join(rel), style_dir.join(rel)];
            let path = candidates.iter().find(|p| p.is_file()).cloned();
            match path {
                Some(p) => {
                    set.shared_path = Some(p.clone());
                    match load_shared(&p) {
                        Ok(sections) => set.shared = sections,
                        Err(e) => tracing::warn!("{}", e),
                    }
                }
                None => {
                    tracing::warn!(
                        "Token file '{}' not found (tried {} and {})",
                        rel,
                        candidates[0].display(),
                        candidates[1].display()
                    );
                    set.shared_path = Some(candidates[0].clone());
                }
            }
        }

        set
    }

    /// True when a string is a token reference (`$name`).
    pub fn is_reference(s: &str) -> bool {
        s.len() > 1 && s.starts_with('$')
    }

    /// Look a token up by name, with or without the leading `$`.
    /// `"section.name"` addresses one shared section explicitly.
    pub fn get(&self, name: &str) -> Option<&toml::Value> {
        let name = name.strip_prefix('$').unwrap_or(name);
        if name.is_empty() {
            return None;
        }

        // Local table first (a local key may legitimately contain a dot).
        if let Some(v) = self.local.get(name) {
            return self.deref_shared(v);
        }

        // Section-qualified: "$color.accent"
        if let Some((section, key)) = name.split_once('.') {
            if let Some((_, table)) = self.shared.iter().find(|(s, _)| s == section) {
                return table.get(key);
            }
        }

        // Every shared section, in file order
        self.shared.iter().find_map(|(_, table)| table.get(name))
    }

    /// A local token that is itself `"$ref"` resolves into the shared file.
    fn deref_shared<'a>(&'a self, v: &'a toml::Value) -> Option<&'a toml::Value> {
        match v {
            toml::Value::String(s) if Self::is_reference(s) => {
                let inner = &s[1..];
                if let Some((section, key)) = inner.split_once('.') {
                    if let Some((_, table)) = self.shared.iter().find(|(s, _)| s == section) {
                        return table.get(key);
                    }
                }
                self.shared.iter().find_map(|(_, table)| table.get(inner))
            }
            other => Some(other),
        }
    }

    /// Resolve a `"$name"` reference used as the value of style property
    /// `prop`. Returns `None` (after warning once per token name) when the
    /// token is unknown or cannot be expressed as a style value.
    pub fn resolve_style(&mut self, prop: &str, reference: &str) -> Option<StyleValue> {
        let value = match self.get(reference) {
            Some(v) => v.clone(),
            None => {
                self.warn_once(reference, prop, "unknown token");
                return None;
            }
        };
        match toml_to_style_value(&value) {
            Some(sv) => Some(sv),
            None => {
                self.warn_once(reference, prop, "token value is not a style value");
                None
            }
        }
    }

    fn warn_once(&mut self, reference: &str, prop: &str, why: &str) {
        if self.warned.insert(reference.to_string()) {
            tracing::warn!(
                "Style token '{}' (used by '{}'): {} — property left unset",
                reference,
                prop,
                why
            );
        }
    }

    /// Every file this set depends on (for hot reload).
    pub fn watched_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.shared_path.iter()
    }
}

/// Parse a shared token file into its sections, preserving file order.
fn load_shared(path: &Path) -> Result<Sections, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read token file {}: {}", path.display(), e))?;
    let doc: toml::Value = content
        .parse()
        .map_err(|e| format!("Failed to parse token file {}: {}", path.display(), e))?;
    let table = doc
        .as_table()
        .ok_or_else(|| format!("Token file {} is not a TOML table", path.display()))?;

    // toml::Value::Table is sorted, not ordered; recover file order from
    // the position of each `[section]` header in the source text.
    let mut sections: Vec<(usize, String, Table)> = table
        .iter()
        .filter_map(|(name, v)| {
            v.as_table().map(|t| {
                let pos = content.find(&format!("[{}]", name)).unwrap_or(usize::MAX);
                (pos, name.clone(), t.clone())
            })
        })
        .collect();
    sections.sort_by_key(|(pos, _, _)| *pos);
    Ok(sections.into_iter().map(|(_, name, t)| (name, t)).collect())
}

/// Convert a TOML value to a StyleValue. Shared by the style loader and the
/// token resolver so a token carries exactly what an inline value would.
pub fn toml_to_style_value(val: &toml::Value) -> Option<StyleValue> {
    match val {
        toml::Value::Float(f) => Some(StyleValue::Float(*f as f32)),
        toml::Value::Integer(i) => Some(StyleValue::Float(*i as f32)),
        toml::Value::String(s) => Some(StyleValue::String(s.clone())),
        toml::Value::Boolean(b) => Some(StyleValue::Bool(*b)),
        toml::Value::Array(arr) => {
            // Color array [r, g, b(, a)], padding [l, t, r, b],
            // or shadow [dx, dy, r, g, b, a]
            let values: Vec<f32> = arr
                .iter()
                .filter_map(|v| match v {
                    toml::Value::Float(f) => Some(*f as f32),
                    toml::Value::Integer(i) => Some(*i as f32),
                    _ => None,
                })
                .collect();
            if values.len() != arr.len() {
                return None;
            }
            StyleValue::from_numbers(&values)
        }
        _ => None,
    }
}
