//! Structure-preserving save for `*.particles.toml`.
//!
//! When the emitter list has the same length, order and names as the saved
//! copy, only the keys whose values changed are patched into the existing
//! `toml_edit` document, so comments and formatting elsewhere survive. When
//! emitters were added, removed, renamed or reordered the file is rewritten
//! from the serialised effect (comments inside it are lost; the status line
//! says so).

use std::path::Path;

use flint_particles::{EmitterDef, ParticleEffect};
use toml_edit::{DocumentMut, Item};

/// How the file was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveMode {
    /// Only changed keys patched; comments preserved.
    Patched,
    /// Whole file regenerated (structure changed).
    Rewritten,
}

impl std::fmt::Display for SaveMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveMode::Patched => write!(f, "patched"),
            SaveMode::Rewritten => write!(f, "rewritten — structure changed"),
        }
    }
}

/// Save `live` over `path`, using `saved` (the last on-disk state) to decide
/// what changed.
pub fn save_effect(
    path: &Path,
    saved: &ParticleEffect,
    live: &ParticleEffect,
) -> Result<SaveMode, String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    match patch_document(&text, saved, live) {
        Ok(Some(doc)) => {
            std::fs::write(path, doc).map_err(|e| e.to_string())?;
            Ok(SaveMode::Patched)
        }
        Ok(None) | Err(_) => {
            let out = live.to_toml_string()?;
            std::fs::write(path, out).map_err(|e| e.to_string())?;
            Ok(SaveMode::Rewritten)
        }
    }
}

/// Produce the patched document text, or `None` when a full rewrite is needed.
pub fn patch_document(
    text: &str,
    saved: &ParticleEffect,
    live: &ParticleEffect,
) -> Result<Option<String>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    if !same_structure(saved, live) {
        return Ok(None);
    }
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;

    // Top level scalars.
    let root = doc.as_table_mut();
    if saved.name != live.name {
        root.insert("name", toml_edit::value(live.name.clone()));
    }
    if saved.seed != live.seed {
        root.insert("seed", toml_edit::value(live.seed as i64));
    }
    if saved.budget != live.budget {
        match live.budget {
            Some(b) => {
                root.insert("budget", toml_edit::value(b as i64));
            }
            None => {
                root.remove("budget");
            }
        }
    }

    // Emitters: the on-disk array of tables must line up with the effect.
    let n = live.emitters.len();
    let emitters = doc
        .get_mut("emitters")
        .and_then(|i| i.as_array_of_tables_mut())
        .ok_or("no [[emitters]] array in document")?;
    if emitters.len() != n {
        return Ok(None);
    }
    for (i, (old, new)) in saved.emitters.iter().zip(live.emitters.iter()).enumerate() {
        if old == new {
            continue;
        }
        let table = emitters.get_mut(i).ok_or("emitter table missing")?;
        let old_v = emitter_table(old)?;
        let new_v = emitter_table(new)?;
        // Union of keys, in a stable order: existing document keys first.
        let mut keys: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
        for k in new_v.keys() {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
        for key in keys {
            let ov = old_v.get(&key);
            let nv = new_v.get(&key);
            if ov == nv {
                continue;
            }
            match nv {
                None => {
                    table.remove(&key);
                }
                Some(v) => {
                    // Replace a plain value in place so its decor (trailing
                    // comment, spacing) survives.
                    if let Some(existing) = table.get_mut(&key).and_then(Item::as_value_mut) {
                        if !matches!(v, toml::Value::Table(_)) {
                            let decor = existing.decor().clone();
                            *existing = to_value(v);
                            *existing.decor_mut() = decor;
                            continue;
                        }
                    }
                    let prefer_table = table
                        .get(&key)
                        .map(|existing| existing.is_table() || existing.is_array_of_tables())
                        .unwrap_or(false);
                    table.insert(&key, to_item(v, prefer_table));
                }
            }
        }
    }
    Ok(Some(doc.to_string()))
}

fn same_structure(a: &ParticleEffect, b: &ParticleEffect) -> bool {
    a.emitters.len() == b.emitters.len()
        && a.emitters
            .iter()
            .zip(b.emitters.iter())
            .all(|(x, y)| x.name == y.name)
}

fn emitter_table(def: &EmitterDef) -> Result<toml::value::Table, String> {
    match toml::Value::try_from(def).map_err(|e| e.to_string())? {
        toml::Value::Table(t) => Ok(t),
        _ => Err("emitter did not serialise to a table".into()),
    }
}

/// Convert a `toml::Value` into a `toml_edit::Item`. Arrays and tables are
/// inline (one line) unless `prefer_table` asks for a standard table /
/// array-of-tables to match what the file already used.
fn to_item(v: &toml::Value, prefer_table: bool) -> Item {
    match v {
        toml::Value::Table(t) if prefer_table => {
            let mut tbl = toml_edit::Table::new();
            for (k, x) in t {
                tbl.insert(k, to_item(x, false));
            }
            Item::Table(tbl)
        }
        toml::Value::Array(items)
            if prefer_table && !items.is_empty() && items.iter().all(|x| x.is_table()) =>
        {
            let mut aot = toml_edit::ArrayOfTables::new();
            for x in items {
                if let toml::Value::Table(t) = x {
                    let mut tbl = toml_edit::Table::new();
                    for (k, y) in t {
                        tbl.insert(k, to_item(y, false));
                    }
                    aot.push(tbl);
                }
            }
            Item::ArrayOfTables(aot)
        }
        _ => Item::Value(to_value(v)),
    }
}

fn to_value(v: &toml::Value) -> toml_edit::Value {
    match v {
        toml::Value::String(s) => toml_edit::Value::from(s.as_str()),
        toml::Value::Integer(i) => toml_edit::Value::from(*i),
        toml::Value::Float(f) => toml_edit::Value::from(*f),
        toml::Value::Boolean(b) => toml_edit::Value::from(*b),
        toml::Value::Datetime(d) => toml_edit::Value::from(d.to_string()),
        toml::Value::Array(items) => {
            let mut arr = toml_edit::Array::new();
            for x in items {
                arr.push(to_value(x));
            }
            toml_edit::Value::Array(arr)
        }
        toml::Value::Table(t) => {
            let mut it = toml_edit::InlineTable::new();
            for (k, x) in t {
                it.insert(k, to_value(x));
            }
            toml_edit::Value::InlineTable(it)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"# header comment stays
name = "fx"
seed = 3

[[emitters]]
name = "a"
emission_rate = 10.0   # rate comment
lifetime = [0.5, 1.0]

[emitters.shape]
kind = "sphere"
radius = 0.5

[[emitters]]
name = "b"
emission_rate = 2.0
"#;

    fn load(text: &str) -> ParticleEffect {
        ParticleEffect::from_toml_str(text, "t").unwrap()
    }

    #[test]
    fn identical_effects_patch_nothing() {
        let fx = load(SRC);
        let out = patch_document(SRC, &fx, &fx).unwrap().unwrap();
        assert_eq!(out, SRC);
    }

    #[test]
    fn scalar_change_preserves_comments() {
        let saved = load(SRC);
        let mut live = saved.clone();
        live.emitters[0].emission_rate = 25.0;
        let out = patch_document(SRC, &saved, &live).unwrap().unwrap();
        assert!(out.contains("# header comment stays"));
        assert!(out.contains("emission_rate = 25.0"));
        assert!(out.contains("# rate comment"), "{out}");
        assert!(
            out.contains("[emitters.shape]"),
            "untouched table kept: {out}"
        );
        assert_eq!(load(&out), live);
    }

    #[test]
    fn table_key_keeps_table_style_and_new_keys_inline() {
        let saved = load(SRC);
        let mut live = saved.clone();
        live.emitters[0].shape =
            flint_particles::effect::ShapeField::Def(flint_particles::ShapeDef::Cone {
                radius: 0.2,
                angle: 12.0,
            });
        live.emitters[1].gravity = [0.0, 1.0, 0.0];
        let out = patch_document(SRC, &saved, &live).unwrap().unwrap();
        assert!(out.contains("[emitters.shape]"), "{out}");
        assert!(out.contains("kind = \"cone\""), "{out}");
        assert!(out.contains("gravity = [0.0, 1.0, 0.0]"), "{out}");
        assert_eq!(load(&out), live);
    }

    #[test]
    fn removed_optional_key_is_deleted() {
        let saved = load(SRC);
        let mut live = saved.clone();
        live.emitters[0].lifetime = None;
        let out = patch_document(SRC, &saved, &live).unwrap().unwrap();
        assert!(!out.contains("lifetime"), "{out}");
    }

    #[test]
    fn structural_change_requests_rewrite() {
        let saved = load(SRC);
        let mut live = saved.clone();
        live.emitters.swap(0, 1);
        assert!(patch_document(SRC, &saved, &live).unwrap().is_none());
        let mut live2 = saved.clone();
        live2.emitters[0].name = "renamed".into();
        assert!(patch_document(SRC, &saved, &live2).unwrap().is_none());
    }

    #[test]
    fn floats_keep_a_decimal_point() {
        let saved = load(SRC);
        let mut live = saved.clone();
        live.emitters[1].emission_rate = 1.0;
        let out = patch_document(SRC, &saved, &live).unwrap().unwrap();
        assert!(out.contains("emission_rate = 1.0"), "{out}");
    }
}
