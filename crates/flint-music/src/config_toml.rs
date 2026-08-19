//! Shared TOML plumbing for every config/content format this crate parses.
//!
//! One place for the read → parse → `schema_version` contract that the seven
//! per-format parsers previously each re-implemented, plus the
//! "missing or malformed" accessor helpers. Error text is unchanged from the
//! per-module originals (each helper takes the format's display name).
//!
//! Two `schema_version` policies exist on purpose — content formats (chart,
//! event script, suite manifest) REQUIRE the key and leave value checks to
//! their validators (golden fixtures pin those error codes), while config
//! files (coherence/gradient/haptics/ladder) default an absent key to 0 and
//! reject non-zero at parse. Unifying them is a behavior change; propose it,
//! don't slip it in here.

use flint_core::toml_util::toml_f64;
use flint_core::{FlintError, Result};

/// How a format treats `schema_version`.
pub(crate) enum VersionPolicy {
    /// Key must be present (content formats); the value is returned untouched.
    Required,
    /// Absent means 0; any non-zero value is rejected here (config files).
    DefaultZeroStrict,
}

/// Parse TOML text and resolve `schema_version` under the format's policy.
/// Every error carries `name` as its prefix, matching the historical text.
pub(crate) fn parse_versioned(
    name: &str,
    text: &str,
    policy: VersionPolicy,
) -> Result<(toml::Value, i64)> {
    let root: toml::Value = text
        .parse()
        .map_err(|e| FlintError::TomlParseError(format!("{name}: {e}")))?;
    let version = root.get("schema_version").and_then(|v| v.as_integer());
    let version = match policy {
        VersionPolicy::Required => version.ok_or_else(|| bad(name, "schema_version"))?,
        VersionPolicy::DefaultZeroStrict => {
            let v = version.unwrap_or(0);
            if v != 0 {
                return Err(FlintError::ValidationError(format!(
                    "{name}: unknown schema_version {v}"
                )));
            }
            v
        }
    };
    Ok((root, version))
}

/// The shared shape-error: `"{name}: missing or malformed `{what}`"`.
pub(crate) fn bad(name: &str, what: &str) -> FlintError {
    FlintError::ParseError(format!("{name}: missing or malformed `{what}`"))
}

// -- accessors over a root value ----------------------------------------------

pub(crate) fn table<'a>(
    name: &str,
    v: &'a toml::Value,
    key: &str,
) -> Result<&'a toml::value::Table> {
    v.get(key)
        .and_then(|v| v.as_table())
        .ok_or_else(|| bad(name, key))
}

pub(crate) fn array<'a>(name: &str, v: &'a toml::Value, key: &str) -> Result<&'a Vec<toml::Value>> {
    v.get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| bad(name, key))
}

/// Array-of-tables, absent or non-array treated as empty (the chart idiom).
pub(crate) fn tables<'a>(root: &'a toml::Value, key: &str) -> Vec<&'a toml::value::Table> {
    root.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_table()).collect())
        .unwrap_or_default()
}

// -- accessors inside a table -------------------------------------------------
// `ctx` is what the error names; callers pass either the key itself or a
// fuller path like `curves[3].beat`.

pub(crate) fn int_in(name: &str, t: &toml::value::Table, key: &str) -> Result<i64> {
    t.get(key)
        .and_then(|v| v.as_integer())
        .ok_or_else(|| bad(name, key))
}

pub(crate) fn float_in(name: &str, t: &toml::value::Table, key: &str, ctx: &str) -> Result<f64> {
    t.get(key).and_then(toml_f64).ok_or_else(|| bad(name, ctx))
}

pub(crate) fn string_in(name: &str, t: &toml::value::Table, key: &str, ctx: &str) -> Result<String> {
    t.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| bad(name, ctx))
}

/// Array of f64 (any element malformed fails the whole array).
pub(crate) fn floats(name: &str, v: Option<&toml::Value>, ctx: &str) -> Result<Vec<f64>> {
    v.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|e| toml_f64(e).ok_or_else(|| bad(name, ctx)))
                .collect()
        })
        .unwrap_or_else(|| Err(bad(name, ctx)))
}

/// `root.[table].key` as f64 with a default — the config-file read idiom.
pub(crate) fn section_f64(root: &toml::Value, table: &str, key: &str, default: f64) -> f64 {
    root.get(table)
        .and_then(|t| t.get(key))
        .and_then(toml_f64)
        .unwrap_or(default)
}
