//! What a texture op is, said once (#720).
//!
//! Each op in [`super::ops`] has an [`OpDescriptor`] beside its struct: its
//! type name, label and summary, the channels it reads and writes, every
//! parameter with its kind, default, range and help line, and the function
//! that builds the op from a `[[params.ops]]` table. Everything else is
//! derived from it: the op's JSON schema ([`OpDescriptor::param_schema`],
//! which the node editor, `lint_params` and the AI prompt read), its default
//! table ([`OpDescriptor::default_params`]), parsing with type, choice and
//! range errors ([`OpParams`]), and the crate README's op catalog
//! ([`op_reference_markdown`]). The registry is [`super::pipeline::OPS`];
//! adding an op is its struct, `impl TextureOp` and descriptor in its family's
//! module under `ops/`
//! plus one line there.
//!
//! A build function reads each parameter through [`OpParams`] by key, and a
//! key it reads must be declared: reading an undeclared one panics, and a
//! test builds every op and checks it read every key it declares, so the
//! schema and the parser can't drift apart.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{json, Value};

use super::ops::{OpPortInfo, TextureOp};
use crate::generators::util::{linear_to_hex, parse_hex_color};
use crate::{ProcGenError, Result};

/// One texture op: what it's called, what it touches, what it takes, and how
/// to build it.
pub struct OpDescriptor {
    /// The `type` a `[[params.ops]]` table names (e.g. `"brick_grid"`).
    pub type_name: &'static str,
    /// The node editor's title (e.g. `"Brick Grid"`).
    pub label: &'static str,
    /// One sentence on what the op does.
    pub summary: &'static str,
    /// Channels it reads; `<key>` is the channel the parameter `key` names.
    pub reads: &'static [&'static str],
    /// Channels it creates or overwrites.
    pub writes: &'static [&'static str],
    /// Channels it reads and writes in place.
    pub modifies: &'static [&'static str],
    /// Every key it takes besides `type`, `id` and `inputs`.
    pub params: &'static [ParamDesc],
    /// Build the op; reads each parameter through the [`OpParams`].
    pub build: fn(&OpParams) -> Result<Box<dyn TextureOp>>,
}

/// One parameter of an op: its key, kind and default, range and help line.
#[derive(Debug, Clone, Copy)]
pub struct ParamDesc {
    pub key: &'static str,
    pub kind: ParamKind,
    /// Inclusive lower bound of a number (or the least count of a list).
    pub min: Option<f64>,
    /// Inclusive upper bound of a number.
    pub max: Option<f64>,
    /// What the parameter does, in a line: the schema's `description`.
    pub help: &'static str,
}

/// What a parameter holds, and its default.
#[derive(Debug, Clone, Copy)]
pub enum ParamKind {
    /// A whole number ≥ 0.
    Int(u32),
    Float(f32),
    Bool(bool),
    /// A channel name (or other text); `None` is optional with no default.
    Text(Option<&'static str>),
    /// One of a fixed set of names.
    Choice {
        options: &'static [&'static str],
        default: &'static str,
    },
    /// A hex colour (`#RRGGBB` or `#RRGGBBAA`), default in linear 0–1 RGBA.
    /// The schema shows the default as its nearest 8-bit hex.
    Color([f32; 4]),
    /// An array of tables (`[[params.ops.<key>]]`), each with these keys;
    /// `required` means at least one.
    Tables {
        title: &'static str,
        items: &'static [ParamDesc],
        required: bool,
    },
}

impl ParamDesc {
    const fn new(key: &'static str, kind: ParamKind) -> Self {
        ParamDesc {
            key,
            kind,
            min: None,
            max: None,
            help: "",
        }
    }
    pub const fn int(key: &'static str, default: u32) -> Self {
        Self::new(key, ParamKind::Int(default))
    }
    pub const fn float(key: &'static str, default: f32) -> Self {
        Self::new(key, ParamKind::Float(default))
    }
    pub const fn flag(key: &'static str, default: bool) -> Self {
        Self::new(key, ParamKind::Bool(default))
    }
    /// A channel name with a default.
    pub const fn channel(key: &'static str, default: &'static str) -> Self {
        Self::new(key, ParamKind::Text(Some(default)))
    }
    /// An optional channel name: unset, the op goes without it.
    pub const fn opt_channel(key: &'static str) -> Self {
        Self::new(key, ParamKind::Text(None))
    }
    pub const fn choice(
        key: &'static str,
        options: &'static [&'static str],
        default: &'static str,
    ) -> Self {
        Self::new(key, ParamKind::Choice { options, default })
    }
    pub const fn color(key: &'static str, default: [f32; 4]) -> Self {
        Self::new(key, ParamKind::Color(default))
    }
    /// A required array of tables, at least one.
    pub const fn tables(
        key: &'static str,
        title: &'static str,
        items: &'static [ParamDesc],
    ) -> Self {
        Self::new(
            key,
            ParamKind::Tables {
                title,
                items,
                required: true,
            },
        )
    }
    pub const fn min(mut self, min: f64) -> Self {
        self.min = Some(min);
        self
    }
    pub const fn max(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }
    pub const fn range(self, min: f64, max: f64) -> Self {
        self.min(min).max(max)
    }
    pub const fn help(mut self, help: &'static str) -> Self {
        self.help = help;
        self
    }
}

// ─── Derived: ports, schema, defaults ─────────────────────────────────────

impl OpDescriptor {
    /// The channels it reads and writes, for the node editor and graph mode.
    pub fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: self.type_name,
            label: self.label,
            reads: self.reads,
            writes: self.writes,
            modifies: self.modifies,
        }
    }

    /// The parameter `key`, if the op takes it.
    pub fn param(&self, key: &str) -> Option<&'static ParamDesc> {
        self.params.iter().find(|p| p.key == key)
    }

    /// Build the op from its `[[params.ops]]` table (entry `index`).
    pub fn parse(
        &self,
        table: &toml::map::Map<String, toml::Value>,
        index: usize,
    ) -> Result<Box<dyn TextureOp>> {
        let params = OpParams::new(table, self.params, self.type_name, format!("ops[{index}]"));
        (self.build)(&params)
    }

    /// The JSON Schema of the op's parameters (`type`, `id` and `inputs`
    /// are added by [`super::pipeline::op_schema`]).
    pub fn param_schema(&self) -> Value {
        let mut schema = object_schema(self.params);
        schema["description"] = json!(self.summary);
        schema
    }

    /// The op's table at its defaults: `type` and every key with a default.
    pub fn default_params(&self) -> toml::map::Map<String, toml::Value> {
        let mut map = toml::map::Map::new();
        map.insert("type".into(), toml::Value::String(self.type_name.into()));
        for p in self.params {
            let value = match p.kind {
                ParamKind::Int(d) => toml::Value::Integer(d as i64),
                ParamKind::Float(d) => toml::Value::Float(shortest(d)),
                ParamKind::Bool(d) => toml::Value::Boolean(d),
                ParamKind::Text(Some(d)) => toml::Value::String(d.into()),
                ParamKind::Choice { default, .. } => toml::Value::String(default.into()),
                ParamKind::Color(c) => toml::Value::String(linear_to_hex(c)),
                ParamKind::Text(None) | ParamKind::Tables { .. } => continue,
            };
            map.insert(p.key.into(), value);
        }
        map
    }
}

/// `{"type": "object", "properties": ..., "required": ...}` for a key list.
fn object_schema(params: &[ParamDesc]) -> Value {
    let properties: serde_json::Map<String, Value> = params
        .iter()
        .map(|p| (p.key.to_string(), param_schema(p)))
        .collect();
    let required: Vec<&str> = params
        .iter()
        .filter(|p| matches!(p.kind, ParamKind::Tables { required: true, .. }))
        .map(|p| p.key)
        .collect();
    let mut schema = json!({ "type": "object", "properties": properties });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

fn param_schema(p: &ParamDesc) -> Value {
    let mut s = match p.kind {
        ParamKind::Int(d) => json!({ "type": "integer", "default": d }),
        ParamKind::Float(d) => json!({ "type": "number", "default": shortest(d) }),
        ParamKind::Bool(d) => json!({ "type": "boolean", "default": d }),
        ParamKind::Text(Some(d)) => json!({ "type": "string", "default": d }),
        ParamKind::Text(None) => json!({ "type": "string" }),
        ParamKind::Choice { options, default } => {
            json!({ "type": "string", "enum": options, "default": default })
        }
        ParamKind::Color(c) => json!({
            "type": "string",
            "pattern": "^#?[0-9a-fA-F]{6,8}$",
            "default": linear_to_hex(c)
        }),
        ParamKind::Tables { title, items, .. } => {
            let mut item = object_schema(items);
            item["title"] = json!(title);
            item["additionalProperties"] = json!(false);
            json!({ "type": "array", "items": item })
        }
    };
    let bound = |v: f64| match p.kind {
        ParamKind::Int(_) => json!(v as i64),
        _ => json!(v),
    };
    if let Some(min) = p.min {
        if matches!(p.kind, ParamKind::Tables { .. }) {
            s["minItems"] = json!(min as u64);
        } else {
            s["minimum"] = bound(min);
        }
    }
    if let Some(max) = p.max {
        s["maximum"] = bound(max);
    }
    s["description"] = json!(p.help);
    s
}

/// An `f32` as the `f64` its shortest decimal reads as: `0.04`, not
/// `0.03999999910593033`.
pub(super) fn shortest(v: f32) -> f64 {
    v.to_string().parse().unwrap_or(v as f64)
}

// ─── Parsing ──────────────────────────────────────────────────────────────

/// An op's (or a list entry's) table, read by key against its declared
/// parameters: an absent key is its default, and a wrong type, an unknown
/// choice or a number out of range is an error naming `ops[i].key`.
pub struct OpParams<'a> {
    table: &'a toml::map::Map<String, toml::Value>,
    params: &'static [ParamDesc],
    owner: &'static str,
    path: String,
    read: Rc<RefCell<Vec<&'static str>>>,
}

impl<'a> OpParams<'a> {
    fn new(
        table: &'a toml::map::Map<String, toml::Value>,
        params: &'static [ParamDesc],
        owner: &'static str,
        path: String,
    ) -> Self {
        OpParams {
            table,
            params,
            owner,
            path,
            read: Rc::default(),
        }
    }

    /// The declaration of `key` and the table's value for it, if set.
    fn get(&self, key: &str) -> (&'static ParamDesc, Option<&'a toml::Value>) {
        let Some(desc) = self.params.iter().find(|p| p.key == key) else {
            panic!(
                "the {} op reads `{key}`, which its descriptor doesn't declare",
                self.owner
            )
        };
        self.read.borrow_mut().push(desc.key);
        (desc, self.table.get(key))
    }

    fn error(&self, key: &str, reason: impl Into<String>) -> ProcGenError {
        ProcGenError::InvalidParameter {
            name: format!("{}.{key}", self.path),
            reason: reason.into(),
        }
    }

    fn in_range(&self, desc: &ParamDesc, n: f64) -> Result<()> {
        let show = |v: f64| shortest(v as f32);
        if let Some(min) = desc.min.filter(|&m| n < m) {
            return Err(self.error(
                desc.key,
                format!("{} is below the minimum {}", show(n), show(min)),
            ));
        }
        if let Some(max) = desc.max.filter(|&m| n > m) {
            return Err(self.error(
                desc.key,
                format!("{} is above the maximum {}", show(n), show(max)),
            ));
        }
        Ok(())
    }

    pub fn u32(&self, key: &str) -> Result<u32> {
        let (desc, value) = self.get(key);
        let ParamKind::Int(default) = desc.kind else {
            panic!("`{key}` of the {} op isn't an integer", self.owner)
        };
        let Some(v) = value else { return Ok(default) };
        let i = v
            .as_integer()
            .ok_or_else(|| self.error(key, "expected an integer"))?;
        if i < 0 {
            return Err(self.error(key, "must be non-negative"));
        }
        let n = u32::try_from(i).map_err(|_| self.error(key, "is too large"))?;
        self.in_range(desc, n as f64)?;
        Ok(n)
    }

    pub fn f32(&self, key: &str) -> Result<f32> {
        let (desc, value) = self.get(key);
        let ParamKind::Float(default) = desc.kind else {
            panic!("`{key}` of the {} op isn't a number", self.owner)
        };
        let Some(v) = value else { return Ok(default) };
        let n = match v {
            toml::Value::Float(f) => *f,
            toml::Value::Integer(i) => *i as f64,
            _ => return Err(self.error(key, "expected a number")),
        };
        self.in_range(desc, n)?;
        Ok(n as f32)
    }

    pub fn flag(&self, key: &str) -> Result<bool> {
        let (desc, value) = self.get(key);
        let ParamKind::Bool(default) = desc.kind else {
            panic!("`{key}` of the {} op isn't a bool", self.owner)
        };
        match value {
            None => Ok(default),
            Some(v) => v
                .as_bool()
                .ok_or_else(|| self.error(key, "expected true or false")),
        }
    }

    /// A text parameter that has a default.
    pub fn text(&self, key: &str) -> Result<String> {
        let (desc, _) = self.get(key);
        let ParamKind::Text(Some(default)) = desc.kind else {
            panic!("`{key}` of the {} op isn't text with a default", self.owner)
        };
        Ok(self.opt_text(key)?.unwrap_or_else(|| default.to_string()))
    }

    /// A text parameter: `None` when unset and it has no default.
    pub fn opt_text(&self, key: &str) -> Result<Option<String>> {
        let (desc, value) = self.get(key);
        let ParamKind::Text(default) = desc.kind else {
            panic!("`{key}` of the {} op isn't text", self.owner)
        };
        match value {
            None => Ok(default.map(String::from)),
            Some(v) => v
                .as_str()
                .map(|s| Some(s.to_string()))
                .ok_or_else(|| self.error(key, "expected a string (a channel name)")),
        }
    }

    /// One of the parameter's options (its default when unset).
    pub fn choice(&self, key: &str) -> Result<&'a str> {
        let (desc, value) = self.get(key);
        let ParamKind::Choice { options, default } = desc.kind else {
            panic!("`{key}` of the {} op isn't a choice", self.owner)
        };
        let Some(v) = value else { return Ok(default) };
        let s = v
            .as_str()
            .ok_or_else(|| self.error(key, "expected a string"))?;
        if !options.contains(&s) {
            return Err(self.error(
                key,
                format!("\"{s}\" is not one of: {}", options.join(", ")),
            ));
        }
        Ok(s)
    }

    /// A colour, linear 0–1 RGBA.
    pub fn color(&self, key: &str) -> Result<[f32; 4]> {
        let (desc, value) = self.get(key);
        let ParamKind::Color(default) = desc.kind else {
            panic!("`{key}` of the {} op isn't a colour", self.owner)
        };
        let Some(v) = value else { return Ok(default) };
        let s = v
            .as_str()
            .ok_or_else(|| self.error(key, "expected a hex color string"))?;
        parse_hex_color(s).map_err(|e| match e {
            ProcGenError::InvalidParameter { reason, .. } => self.error(key, reason),
            other => other,
        })
    }

    /// The entries of an array-of-tables parameter, each read against its
    /// item keys.
    pub fn tables(&self, key: &str) -> Result<Vec<OpParams<'a>>> {
        let (desc, value) = self.get(key);
        let ParamKind::Tables {
            title,
            items,
            required,
        } = desc.kind
        else {
            panic!("`{key}` of the {} op isn't a list of tables", self.owner)
        };
        let entries = match value {
            None if required => {
                return Err(self.error(
                    key,
                    format!("{} requires a [[params.ops.{key}]] array", self.owner),
                ))
            }
            None => return Ok(Vec::new()),
            Some(v) => v
                .as_array()
                .ok_or_else(|| self.error(key, "expected an array of tables"))?,
        };
        if required && entries.is_empty() {
            return Err(self.error(key, format!("{} needs at least one entry", self.owner)));
        }
        entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let path = format!("{}.{key}[{i}]", self.path);
                let table = entry
                    .as_table()
                    .ok_or_else(|| ProcGenError::InvalidParameter {
                        name: path.clone(),
                        reason: "each entry must be a TOML table".into(),
                    })?;
                Ok(OpParams {
                    table,
                    params: items,
                    owner: title,
                    path,
                    read: self.read.clone(),
                })
            })
            .collect()
    }

    /// A reader of `op`'s table, as [`OpDescriptor::parse`] makes.
    #[cfg(test)]
    pub(crate) fn for_test(
        table: &'a toml::map::Map<String, toml::Value>,
        op: &OpDescriptor,
    ) -> Self {
        Self::new(table, op.params, op.type_name, "ops[0]".into())
    }

    /// Every key read so far, here and in the entries [`Self::tables`] gave.
    #[cfg(test)]
    pub(crate) fn keys_read(&self) -> Vec<&'static str> {
        let mut keys = self.read.borrow().clone();
        keys.sort_unstable();
        keys.dedup();
        keys
    }
}

// ─── The catalog ──────────────────────────────────────────────────────────

/// The crate README's op catalog: each op in registry order with its
/// summary, channels and a table of its parameters.
pub fn op_reference_markdown() -> String {
    let channels = |names: &[&str]| {
        names
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out = String::from(
        "A channel in angle brackets is the one the parameter of that name names \
         (`<output>` is the channel `output` says).\n",
    );
    for op in super::pipeline::OPS {
        out.push_str(&format!(
            "\n### `{}` ({})\n\n{}\n\n",
            op.type_name, op.label, op.summary
        ));
        let mut io = Vec::new();
        for (verb, names) in [
            ("Reads", op.reads),
            ("writes", op.writes),
            ("modifies", op.modifies),
        ] {
            if !names.is_empty() {
                io.push(format!("{verb} {}", channels(names)));
            }
        }
        if !io.is_empty() {
            let mut line = io.join("; ");
            line[..1].make_ascii_uppercase();
            out.push_str(&format!("{line}.\n\n"));
        }
        // A `tables` key's entries follow, as `[[params.ops.<key>]]`
        out.push_str(&crate::param_reference_markdown_in(
            &op.param_schema(),
            "params.ops",
        ));
    }
    out
}
