//! One value set in, or taken out of, a table of a scene file.
//!
//! A [`SceneEdit`] names a table by its header and a key by its dotted path
//! inside it; [`SceneDocument::apply_edit`](crate::SceneDocument::apply_edit)
//! lands it, changing only that value's text: comments, order, inline tables
//! and everything else in the file stay as they were. What a feature writes
//! back to a scene is a list of these (living props' tuned look, #249:
//! `flint_procgen::commit_look`).

use std::fmt;

/// One value a scene file can hold.
#[derive(Debug, Clone, PartialEq)]
pub enum SceneScalar {
    /// Written as briefly as reads back to the same bits.
    Float(f32),
    Int(i64),
    Bool(bool),
    Text(String),
}

impl SceneScalar {
    /// The value as `toml_edit` writes it.
    pub(crate) fn to_edit(&self) -> toml_edit::Value {
        match self {
            SceneScalar::Float(value) => format!("{value:?}")
                .parse::<toml_edit::Value>()
                .unwrap_or_else(|_| toml_edit::Value::from(f64::from(*value))),
            SceneScalar::Int(value) => toml_edit::Value::from(*value),
            SceneScalar::Bool(value) => toml_edit::Value::from(*value),
            SceneScalar::Text(value) => toml_edit::Value::from(value.as_str()),
        }
    }

    /// Whether a value already in the file says this, however it's written: so a
    /// commit leaves `step = 0.50` alone where the look says 0.5.
    pub(crate) fn is_in(&self, there: &toml_edit::Value) -> bool {
        match (self, there) {
            (SceneScalar::Float(value), toml_edit::Value::Float(f)) => *f.value() as f32 == *value,
            (SceneScalar::Float(value), toml_edit::Value::Integer(i)) => {
                *i.value() as f32 == *value
            }
            (SceneScalar::Int(value), toml_edit::Value::Integer(i)) => i.value() == value,
            (SceneScalar::Bool(value), toml_edit::Value::Boolean(b)) => b.value() == value,
            (SceneScalar::Text(value), toml_edit::Value::String(s)) => s.value() == value,
            _ => false,
        }
    }
}

impl fmt::Display for SceneScalar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SceneScalar::Float(value) => write!(f, "{value:?}"),
            SceneScalar::Int(value) => write!(f, "{value}"),
            SceneScalar::Bool(value) => write!(f, "{value}"),
            SceneScalar::Text(value) => write!(f, "\"{value}\""),
        }
    }
}

/// One value to set, or with `None` to take out, in a table of a scene file:
/// `[living.species.grove_thorn]`'s `echoes.count`, say. `table` is the header,
/// and `key` the dotted path inside it, through inline tables.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneEdit {
    pub table: Vec<String>,
    pub key: Vec<String>,
    pub value: Option<SceneScalar>,
}

impl SceneEdit {
    /// How the panel and a log name it: `[living.species.grove_thorn] echoes.count`.
    pub fn label(&self) -> String {
        format!("[{}] {}", self.table.join("."), self.key.join("."))
    }
}

impl fmt::Display for SceneEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.value {
            Some(value) => write!(f, "{} = {value}", self.label()),
            None => write!(f, "{} taken out", self.label()),
        }
    }
}
